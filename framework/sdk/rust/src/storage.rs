//! Optional framework-managed tabular storage (side-channel).
//!
//! Domain provenance stays on the kernel ledger. This module is for product
//! tables the host can query (analytics, step chrome data, export).
//!
//! Lifecycle (when [`StoragePlan`] is not [`StorageMode::Off`](StorageMode::Off)):
//!
//! ```text
//! register_plugin  → remember module TableSchema (if any)
//! start_pipeline   → ensure_table (qualified for PerRun)
//! try_step success → on_store → write_rows (+ optional step_log)
//! run end / cancel → drop PerRun tables when retention says so
//! ```

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::FrameworkError;

/// Contract-ish label for framework step audit rows (not a kernel artifact type).
pub const STEP_LOG_TABLE: &str = "_srcport_step_log";

// ── Policy ──────────────────────────────────────────────────────────────────

/// How framework tables are namespaced and retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageMode {
    /// No framework-managed tables (default). Kernel artifacts still apply.
    #[default]
    Off,
    /// Isolated tables per run: physical name `{run_id}__{module}__{logical}`.
    PerRun,
    /// Durable tables keyed by module logical name; rows carry `_run_id`.
    Shared,
}

/// What to do with [`StorageMode::PerRun`] tables when a run ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageRetention {
    /// Drop per-run tables when the run leaves `RUNNING` (complete/cancel/fail).
    #[default]
    DropOnEnd,
    /// Keep tables after the run (caller / ops must clean up).
    Keep,
}

/// Opinionated storage phase for one pipeline (host-only; never enters the kernel).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StoragePlan {
    /// Table namespacing mode.
    pub mode: StorageMode,
    /// Also append framework-owned step audit rows (`_srcport_step_log`).
    pub step_log: bool,
    /// Retention for [`StorageMode::PerRun`] tables.
    pub retention: StorageRetention,
}

impl StoragePlan {
    /// No storage phase.
    pub fn off() -> Self {
        Self::default()
    }

    /// Per-run tables; drop when the run ends.
    pub fn per_run() -> Self {
        Self {
            mode: StorageMode::PerRun,
            step_log: false,
            retention: StorageRetention::DropOnEnd,
        }
    }

    /// Per-run tables; keep after end.
    pub fn per_run_keep() -> Self {
        Self {
            mode: StorageMode::PerRun,
            step_log: false,
            retention: StorageRetention::Keep,
        }
    }

    /// Shared durable module tables across runs.
    pub fn shared() -> Self {
        Self {
            mode: StorageMode::Shared,
            step_log: false,
            retention: StorageRetention::Keep,
        }
    }

    /// Only the framework step audit log (no module tables required).
    pub fn step_log_only() -> Self {
        Self {
            mode: StorageMode::Off,
            step_log: true,
            retention: StorageRetention::Keep,
        }
    }

    /// Enable module tables in per-run mode + step log.
    pub fn per_run_with_step_log() -> Self {
        Self {
            mode: StorageMode::PerRun,
            step_log: true,
            retention: StorageRetention::DropOnEnd,
        }
    }

    /// Builder: also write step audit rows.
    pub fn with_step_log(mut self) -> Self {
        self.step_log = true;
        self
    }

    /// Builder: retention for per-run tables.
    pub fn with_retention(mut self, retention: StorageRetention) -> Self {
        self.retention = retention;
        self
    }

    /// True when the host should open/ensure any tables for this plan.
    pub fn enabled(&self) -> bool {
        self.mode != StorageMode::Off || self.step_log
    }

    /// True when module `storage_schema` / `on_store` participate.
    pub fn module_tables(&self) -> bool {
        matches!(self.mode, StorageMode::PerRun | StorageMode::Shared)
    }
}

// ── Schema & values ─────────────────────────────────────────────────────────

/// Column scalar type (opinionated, SQL-friendly).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    /// UTF-8 text.
    Text,
    /// 64-bit signed integer.
    Integer,
    /// IEEE-754 float.
    Real,
    /// Boolean.
    Boolean,
    /// Nested JSON value.
    Json,
    /// Opaque bytes.
    Bytes,
}

/// One column in a module-declared table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnDef {
    /// Column name (stable identifier).
    pub name: String,
    /// Value type.
    pub ty: ColumnType,
    /// Whether NULL is allowed.
    pub nullable: bool,
}

impl ColumnDef {
    /// Required column.
    pub fn required(name: impl Into<String>, ty: ColumnType) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable: false,
        }
    }

    /// Nullable column.
    pub fn optional(name: impl Into<String>, ty: ColumnType) -> Self {
        Self {
            name: name.into(),
            ty,
            nullable: true,
        }
    }
}

/// How rows are applied to a table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteMode {
    /// Append rows (default).
    #[default]
    Append,
    /// Insert or replace by primary key.
    Upsert,
    /// Replace prior rows for this run, then insert.
    ///
    /// - [`StorageMode::PerRun`]: truncate the whole table, then insert.
    /// - [`StorageMode::Shared`]: delete rows with matching `_run_id`, then insert.
    Replace,
}

/// Module-declared logical table (columns + default write semantics).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TableSchema {
    /// Logical table name (module-local). Framework may qualify for PerRun.
    pub name: String,
    /// User columns (framework may add `_run_id`, `_work_id`, …).
    pub columns: Vec<ColumnDef>,
    /// Primary key column names (required for reliable [`WriteMode::Upsert`]).
    pub primary_key: Vec<String>,
    /// Default write mode when [`StoreWrite::mode`] is `None`.
    pub write_mode: WriteMode,
}

impl TableSchema {
    /// Builder helper.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            columns: Vec::new(),
            primary_key: Vec::new(),
            write_mode: WriteMode::Append,
        }
    }

    /// Add a column.
    pub fn column(mut self, col: ColumnDef) -> Self {
        self.columns.push(col);
        self
    }

    /// Set primary key.
    pub fn primary_key(mut self, keys: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.primary_key = keys.into_iter().map(Into::into).collect();
        self
    }

    /// Set default write mode.
    pub fn write_mode(mut self, mode: WriteMode) -> Self {
        self.write_mode = mode;
        self
    }
}

/// Cell value in a store row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StoreValue {
    /// SQL NULL.
    Null,
    /// Boolean.
    Boolean(bool),
    /// Integer.
    Integer(i64),
    /// Float.
    Real(f64),
    /// Text.
    Text(String),
    /// JSON.
    Json(serde_json::Value),
    /// Bytes (serde as base64-ish array of numbers when needed; tests use in-memory).
    Bytes(Vec<u8>),
}

impl From<&str> for StoreValue {
    fn from(s: &str) -> Self {
        StoreValue::Text(s.into())
    }
}

impl From<String> for StoreValue {
    fn from(s: String) -> Self {
        StoreValue::Text(s)
    }
}

impl From<i64> for StoreValue {
    fn from(n: i64) -> Self {
        StoreValue::Integer(n)
    }
}

impl From<bool> for StoreValue {
    fn from(b: bool) -> Self {
        StoreValue::Boolean(b)
    }
}

/// One table row: column name → value.
pub type StoreRow = BTreeMap<String, StoreValue>;

/// Module decision for what to write after a step.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StoreWrite {
    /// Override schema default write mode.
    pub mode: Option<WriteMode>,
    /// Rows to apply (empty = no-op write).
    pub rows: Vec<StoreRow>,
}

impl StoreWrite {
    /// Append these rows.
    pub fn append(rows: impl IntoIterator<Item = StoreRow>) -> Self {
        Self {
            mode: Some(WriteMode::Append),
            rows: rows.into_iter().collect(),
        }
    }

    /// Upsert these rows.
    pub fn upsert(rows: impl IntoIterator<Item = StoreRow>) -> Self {
        Self {
            mode: Some(WriteMode::Upsert),
            rows: rows.into_iter().collect(),
        }
    }

    /// Replace prior run rows, then insert.
    pub fn replace(rows: impl IntoIterator<Item = StoreRow>) -> Self {
        Self {
            mode: Some(WriteMode::Replace),
            rows: rows.into_iter().collect(),
        }
    }

    /// Single-row helper.
    pub fn row(mode: WriteMode, row: StoreRow) -> Self {
        Self {
            mode: Some(mode),
            rows: vec![row],
        }
    }
}

/// Build a row from key/value pairs.
pub fn store_row(
    pairs: impl IntoIterator<Item = (impl Into<String>, StoreValue)>,
) -> StoreRow {
    pairs
        .into_iter()
        .map(|(k, v)| (k.into(), v))
        .collect()
}

// ── Qualification ───────────────────────────────────────────────────────────

/// Physical table identity after policy qualification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QualifiedTable {
    /// Backend table name.
    pub physical_name: String,
    /// Owning module name (empty for pure framework tables).
    pub module: String,
    /// Logical name from the schema.
    pub logical_name: String,
    /// Schema including framework identity columns.
    pub schema: TableSchema,
    /// Mode used when ensuring this table.
    pub mode: StorageMode,
}

/// Framework-injected identity columns (always present on module tables).
pub fn identity_columns() -> Vec<ColumnDef> {
    vec![
        ColumnDef::required("_run_id", ColumnType::Text),
        ColumnDef::required("_work_id", ColumnType::Text),
        ColumnDef::required("_node_id", ColumnType::Text),
        ColumnDef::required("_module", ColumnType::Text),
    ]
}

/// Merge module schema with identity columns (identity first; skip duplicates).
pub fn with_identity_columns(schema: &TableSchema) -> TableSchema {
    let mut columns = identity_columns();
    for c in &schema.columns {
        if !columns.iter().any(|x| x.name == c.name) {
            columns.push(c.clone());
        }
    }
    TableSchema {
        name: schema.name.clone(),
        columns,
        primary_key: schema.primary_key.clone(),
        write_mode: schema.write_mode,
    }
}

/// Qualify a logical schema for a run / mode.
pub fn qualify_table(
    mode: StorageMode,
    run_id: &str,
    module: &str,
    schema: &TableSchema,
) -> QualifiedTable {
    let enriched = with_identity_columns(schema);
    let physical_name = match mode {
        StorageMode::Off => schema.name.clone(),
        StorageMode::PerRun => format!("{run_id}__{module}__{}", schema.name),
        StorageMode::Shared => format!("{module}__{}", schema.name),
    };
    QualifiedTable {
        physical_name,
        module: module.into(),
        logical_name: schema.name.clone(),
        schema: enriched,
        mode,
    }
}

/// Step-log table (framework-owned).
pub fn step_log_qualified(mode: StorageMode, run_id: &str) -> QualifiedTable {
    let schema = TableSchema {
        name: STEP_LOG_TABLE.into(),
        columns: vec![
            ColumnDef::required("run_id", ColumnType::Text),
            ColumnDef::required("work_id", ColumnType::Text),
            ColumnDef::required("node_id", ColumnType::Text),
            ColumnDef::required("module", ColumnType::Text),
            ColumnDef::required("capability", ColumnType::Text),
            ColumnDef::required("ok", ColumnType::Boolean),
            ColumnDef::optional("error", ColumnType::Text),
            ColumnDef::optional("output_ports", ColumnType::Json),
        ],
        primary_key: Vec::new(),
        write_mode: WriteMode::Append,
    };
    let physical_name = match mode {
        // step_log with Off mode uses a global log table
        StorageMode::Off | StorageMode::Shared => STEP_LOG_TABLE.into(),
        StorageMode::PerRun => format!("{run_id}__{STEP_LOG_TABLE}"),
    };
    QualifiedTable {
        physical_name,
        module: String::new(),
        logical_name: STEP_LOG_TABLE.into(),
        schema,
        mode: if mode == StorageMode::Off {
            StorageMode::Shared
        } else {
            mode
        },
    }
}

/// Inject identity fields into a row when missing.
pub fn inject_identity(
    row: &mut StoreRow,
    run_id: &str,
    work_id: &str,
    node_id: &str,
    module: &str,
) {
    row.entry("_run_id".into())
        .or_insert_with(|| StoreValue::Text(run_id.into()));
    row.entry("_work_id".into())
        .or_insert_with(|| StoreValue::Text(work_id.into()));
    row.entry("_node_id".into())
        .or_insert_with(|| StoreValue::Text(node_id.into()));
    row.entry("_module".into())
        .or_insert_with(|| StoreValue::Text(module.into()));
}

// ── Backend ─────────────────────────────────────────────────────────────────

/// Pluggable tabular store (memory for tests; SQL adapters later).
pub trait StorageBackend: Send {
    /// Create table if missing (idempotent). Existing schema mismatches → error.
    fn ensure_table(&mut self, table: &QualifiedTable) -> Result<(), FrameworkError>;

    /// Apply rows with the given write mode.
    ///
    /// `run_id` is used for [`WriteMode::Replace`] under shared tables.
    fn write_rows(
        &mut self,
        table: &str,
        mode: WriteMode,
        rows: &[StoreRow],
        primary_key: &[String],
        run_id: &str,
    ) -> Result<(), FrameworkError>;

    /// Drop a physical table if it exists.
    fn drop_table(&mut self, table: &str) -> Result<(), FrameworkError>;

    /// Whether a physical table exists.
    fn table_exists(&self, table: &str) -> bool;

    /// Snapshot rows for tests / debug (default: empty).
    fn rows(&self, _table: &str) -> Option<Vec<StoreRow>> {
        None
    }

    /// List physical table names (default: empty).
    fn table_names(&self) -> Vec<String> {
        Vec::new()
    }
}

#[derive(Debug, Clone)]
struct MemTable {
    schema: TableSchema,
    mode: StorageMode,
    rows: Vec<StoreRow>,
}

/// In-memory [`StorageBackend`] for tests and light hosts.
#[derive(Debug, Default)]
pub struct MemoryStorage {
    tables: HashMap<String, MemTable>,
}

impl MemoryStorage {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Borrow rows (test helper).
    pub fn get_rows(&self, table: &str) -> Option<&[StoreRow]> {
        self.tables.get(table).map(|t| t.rows.as_slice())
    }
}

impl StorageBackend for MemoryStorage {
    fn ensure_table(&mut self, table: &QualifiedTable) -> Result<(), FrameworkError> {
        if let Some(existing) = self.tables.get(&table.physical_name) {
            if existing.schema.columns != table.schema.columns {
                return Err(FrameworkError::Invalid(format!(
                    "storage schema mismatch for table {}",
                    table.physical_name
                )));
            }
            return Ok(());
        }
        self.tables.insert(
            table.physical_name.clone(),
            MemTable {
                schema: table.schema.clone(),
                mode: table.mode,
                rows: Vec::new(),
            },
        );
        Ok(())
    }

    fn write_rows(
        &mut self,
        table: &str,
        mode: WriteMode,
        rows: &[StoreRow],
        primary_key: &[String],
        run_id: &str,
    ) -> Result<(), FrameworkError> {
        let t = self.tables.get_mut(table).ok_or_else(|| {
            FrameworkError::Invalid(format!("storage table not ensured: {table}"))
        })?;

        match mode {
            WriteMode::Append => {
                t.rows.extend(rows.iter().cloned());
            }
            WriteMode::Replace => {
                match t.mode {
                    StorageMode::PerRun | StorageMode::Off => {
                        t.rows.clear();
                    }
                    StorageMode::Shared => {
                        t.rows.retain(|r| {
                            !matches!(
                                r.get("_run_id").or_else(|| r.get("run_id")),
                                Some(StoreValue::Text(id)) if id == run_id
                            )
                        });
                    }
                }
                t.rows.extend(rows.iter().cloned());
            }
            WriteMode::Upsert => {
                if primary_key.is_empty() {
                    return Err(FrameworkError::Invalid(format!(
                        "upsert on {table} requires primary_key on TableSchema"
                    )));
                }
                for row in rows {
                    if let Some(idx) = t.rows.iter().position(|existing| {
                        primary_key.iter().all(|k| existing.get(k) == row.get(k))
                    }) {
                        t.rows[idx] = row.clone();
                    } else {
                        t.rows.push(row.clone());
                    }
                }
            }
        }
        Ok(())
    }

    fn drop_table(&mut self, table: &str) -> Result<(), FrameworkError> {
        self.tables.remove(table);
        Ok(())
    }

    fn table_exists(&self, table: &str) -> bool {
        self.tables.contains_key(table)
    }

    fn rows(&self, table: &str) -> Option<Vec<StoreRow>> {
        self.tables.get(table).map(|t| t.rows.clone())
    }

    fn table_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self.tables.keys().cloned().collect();
        names.sort();
        names
    }
}
