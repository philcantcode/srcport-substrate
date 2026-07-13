package framework

import (
	"fmt"
	"sort"
	"sync"
)

const StepLogTable = "_srcport_step_log"

// StorageMode namespacing for framework tables.
type StorageMode string

const (
	StorageModeOff    StorageMode = "off"
	StorageModePerRun StorageMode = "per_run"
	StorageModeShared StorageMode = "shared"
)

// StorageRetention for PerRun tables when a run ends.
type StorageRetention string

const (
	RetentionDropOnEnd StorageRetention = "drop_on_end"
	RetentionKeep      StorageRetention = "keep"
)

// StoragePlan is the host-only storage phase for one pipeline.
type StoragePlan struct {
	Mode      StorageMode
	StepLog   bool
	Retention StorageRetention
}

func StorageOff() StoragePlan { return StoragePlan{Mode: StorageModeOff, Retention: RetentionKeep} }
func StoragePerRun() StoragePlan {
	return StoragePlan{Mode: StorageModePerRun, Retention: RetentionDropOnEnd}
}
func StoragePerRunKeep() StoragePlan {
	return StoragePlan{Mode: StorageModePerRun, Retention: RetentionKeep}
}
func StorageShared() StoragePlan {
	return StoragePlan{Mode: StorageModeShared, Retention: RetentionKeep}
}
func StorageStepLogOnly() StoragePlan {
	return StoragePlan{Mode: StorageModeOff, StepLog: true, Retention: RetentionKeep}
}

func (p StoragePlan) WithStepLog() StoragePlan { p.StepLog = true; return p }
func (p StoragePlan) Enabled() bool            { return p.Mode != StorageModeOff || p.StepLog }
func (p StoragePlan) ModuleTables() bool {
	return p.Mode == StorageModePerRun || p.Mode == StorageModeShared
}

// ColumnType is a SQL-friendly scalar type.
type ColumnType string

const (
	ColText    ColumnType = "text"
	ColInteger ColumnType = "integer"
	ColReal    ColumnType = "real"
	ColBoolean ColumnType = "boolean"
	ColJSON    ColumnType = "json"
	ColBytes   ColumnType = "bytes"
)

type ColumnDef struct {
	Name     string
	Type     ColumnType
	Nullable bool
}

func ColRequired(name string, ty ColumnType) ColumnDef {
	return ColumnDef{Name: name, Type: ty, Nullable: false}
}
func ColOptional(name string, ty ColumnType) ColumnDef {
	return ColumnDef{Name: name, Type: ty, Nullable: true}
}

// WriteMode how rows are applied.
type WriteMode string

const (
	WriteAppend  WriteMode = "append"
	WriteUpsert  WriteMode = "upsert"
	WriteReplace WriteMode = "replace"
)

// TableSchema is a module-declared logical table.
type TableSchema struct {
	Name       string
	Columns    []ColumnDef
	PrimaryKey []string
	WriteMode  WriteMode
}

// StoreValue is a cell value (simplified: Go any).
type StoreValue any

// StoreRow is column → value.
type StoreRow map[string]StoreValue

// StoreWrite is what a plugin wants written after a step.
type StoreWrite struct {
	Mode *WriteMode
	Rows []StoreRow
}

func StoreAppend(rows ...StoreRow) *StoreWrite {
	m := WriteAppend
	return &StoreWrite{Mode: &m, Rows: rows}
}

// QualifiedTable is a physical table after policy qualification.
type QualifiedTable struct {
	PhysicalName string
	Module       string
	LogicalName  string
	Schema       TableSchema
	Mode         StorageMode
}

func WithIdentityColumns(schema TableSchema) TableSchema {
	cols := []ColumnDef{
		ColRequired("_run_id", ColText),
		ColRequired("_work_id", ColText),
		ColRequired("_node_id", ColText),
		ColRequired("_module", ColText),
	}
	for _, c := range schema.Columns {
		dup := false
		for _, x := range cols {
			if x.Name == c.Name {
				dup = true
				break
			}
		}
		if !dup {
			cols = append(cols, c)
		}
	}
	schema.Columns = cols
	return schema
}

func QualifyTable(mode StorageMode, runID, module string, schema TableSchema) QualifiedTable {
	enriched := WithIdentityColumns(schema)
	var physical string
	switch mode {
	case StorageModePerRun:
		physical = runID + "__" + module + "__" + schema.Name
	case StorageModeShared:
		physical = module + "__" + schema.Name
	default:
		physical = schema.Name
	}
	return QualifiedTable{
		PhysicalName: physical,
		Module:       module,
		LogicalName:  schema.Name,
		Schema:       enriched,
		Mode:         mode,
	}
}

func StepLogQualified(mode StorageMode, runID string) QualifiedTable {
	schema := TableSchema{
		Name: StepLogTable,
		Columns: []ColumnDef{
			ColRequired("run_id", ColText),
			ColRequired("work_id", ColText),
			ColRequired("node_id", ColText),
			ColRequired("module", ColText),
			ColRequired("capability", ColText),
			ColRequired("ok", ColBoolean),
			ColOptional("error", ColText),
			ColOptional("output_ports", ColJSON),
		},
		WriteMode: WriteAppend,
	}
	physical := StepLogTable
	if mode == StorageModePerRun {
		physical = runID + "__" + StepLogTable
	}
	qm := mode
	if mode == StorageModeOff {
		qm = StorageModeShared
	}
	return QualifiedTable{
		PhysicalName: physical,
		LogicalName:  StepLogTable,
		Schema:       schema,
		Mode:         qm,
	}
}

func InjectIdentity(row StoreRow, runID, workID, nodeID, module string) {
	if _, ok := row["_run_id"]; !ok {
		row["_run_id"] = runID
	}
	if _, ok := row["_work_id"]; !ok {
		row["_work_id"] = workID
	}
	if _, ok := row["_node_id"]; !ok {
		row["_node_id"] = nodeID
	}
	if _, ok := row["_module"]; !ok {
		row["_module"] = module
	}
}

// StorageBackend is a pluggable tabular store.
type StorageBackend interface {
	EnsureTable(table QualifiedTable) error
	WriteRows(table string, mode WriteMode, rows []StoreRow, primaryKey []string, runID string) error
	DropTable(table string) error
	TableExists(table string) bool
	Rows(table string) []StoreRow
	TableNames() []string
}

type memTable struct {
	schema TableSchema
	mode   StorageMode
	rows   []StoreRow
}

// MemoryStorage is an in-memory StorageBackend.
type MemoryStorage struct {
	mu     sync.Mutex
	tables map[string]*memTable
}

func NewMemoryStorage() *MemoryStorage {
	return &MemoryStorage{tables: map[string]*memTable{}}
}

func (s *MemoryStorage) EnsureTable(table QualifiedTable) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	if existing, ok := s.tables[table.PhysicalName]; ok {
		if !columnsEqual(existing.schema.Columns, table.Schema.Columns) {
			return invalid(fmt.Sprintf("storage schema mismatch for table %s", table.PhysicalName))
		}
		return nil
	}
	s.tables[table.PhysicalName] = &memTable{
		schema: table.Schema,
		mode:   table.Mode,
		rows:   nil,
	}
	return nil
}

func (s *MemoryStorage) WriteRows(table string, mode WriteMode, rows []StoreRow, primaryKey []string, runID string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	t, ok := s.tables[table]
	if !ok {
		return invalid(fmt.Sprintf("storage table not ensured: %s", table))
	}
	switch mode {
	case WriteAppend, "":
		for _, r := range rows {
			t.rows = append(t.rows, cloneRow(r))
		}
	case WriteReplace:
		if t.mode == StorageModeShared {
			filtered := t.rows[:0]
			for _, r := range t.rows {
				id, _ := r["_run_id"].(string)
				if id == "" {
					id, _ = r["run_id"].(string)
				}
				if id != runID {
					filtered = append(filtered, r)
				}
			}
			t.rows = filtered
		} else {
			t.rows = nil
		}
		for _, r := range rows {
			t.rows = append(t.rows, cloneRow(r))
		}
	case WriteUpsert:
		if len(primaryKey) == 0 {
			return invalid(fmt.Sprintf("upsert on %s requires primary_key on TableSchema", table))
		}
		for _, row := range rows {
			idx := -1
			for i, existing := range t.rows {
				match := true
				for _, k := range primaryKey {
					if existing[k] != row[k] {
						match = false
						break
					}
				}
				if match {
					idx = i
					break
				}
			}
			if idx >= 0 {
				t.rows[idx] = cloneRow(row)
			} else {
				t.rows = append(t.rows, cloneRow(row))
			}
		}
	default:
		return invalid(fmt.Sprintf("unknown write mode %q", mode))
	}
	return nil
}

func (s *MemoryStorage) DropTable(table string) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	delete(s.tables, table)
	return nil
}

func (s *MemoryStorage) TableExists(table string) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	_, ok := s.tables[table]
	return ok
}

func (s *MemoryStorage) Rows(table string) []StoreRow {
	s.mu.Lock()
	defer s.mu.Unlock()
	t, ok := s.tables[table]
	if !ok {
		return nil
	}
	out := make([]StoreRow, len(t.rows))
	for i, r := range t.rows {
		out[i] = cloneRow(r)
	}
	return out
}

func (s *MemoryStorage) TableNames() []string {
	s.mu.Lock()
	defer s.mu.Unlock()
	names := make([]string, 0, len(s.tables))
	for n := range s.tables {
		names = append(names, n)
	}
	sort.Strings(names)
	return names
}

func cloneRow(r StoreRow) StoreRow {
	cp := make(StoreRow, len(r))
	for k, v := range r {
		cp[k] = v
	}
	return cp
}

func columnsEqual(a, b []ColumnDef) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}
