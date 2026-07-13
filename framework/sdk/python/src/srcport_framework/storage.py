"""Optional framework-managed tabular storage (side-channel)."""

from __future__ import annotations

from dataclasses import dataclass, field
from enum import Enum
from typing import Any, Protocol

from .errors import invalid

STEP_LOG_TABLE = "_srcport_step_log"

StoreValue = Any
StoreRow = dict[str, StoreValue]


class StorageMode(str, Enum):
    OFF = "off"
    PER_RUN = "per_run"
    SHARED = "shared"


class StorageRetention(str, Enum):
    DROP_ON_END = "drop_on_end"
    KEEP = "keep"


class ColumnType(str, Enum):
    TEXT = "text"
    INTEGER = "integer"
    REAL = "real"
    BOOLEAN = "boolean"
    JSON = "json"
    BYTES = "bytes"


class WriteMode(str, Enum):
    APPEND = "append"
    UPSERT = "upsert"
    REPLACE = "replace"


@dataclass
class StoragePlan:
    mode: StorageMode = StorageMode.OFF
    step_log: bool = False
    retention: StorageRetention = StorageRetention.KEEP

    @staticmethod
    def off() -> StoragePlan:
        return StoragePlan()

    @staticmethod
    def per_run() -> StoragePlan:
        return StoragePlan(mode=StorageMode.PER_RUN, retention=StorageRetention.DROP_ON_END)

    @staticmethod
    def per_run_keep() -> StoragePlan:
        return StoragePlan(mode=StorageMode.PER_RUN, retention=StorageRetention.KEEP)

    @staticmethod
    def shared() -> StoragePlan:
        return StoragePlan(mode=StorageMode.SHARED, retention=StorageRetention.KEEP)

    @staticmethod
    def step_log_only() -> StoragePlan:
        return StoragePlan(mode=StorageMode.OFF, step_log=True, retention=StorageRetention.KEEP)

    def with_step_log(self) -> StoragePlan:
        self.step_log = True
        return self

    def enabled(self) -> bool:
        return self.mode != StorageMode.OFF or self.step_log

    def module_tables(self) -> bool:
        return self.mode in (StorageMode.PER_RUN, StorageMode.SHARED)


@dataclass
class ColumnDef:
    name: str
    ty: ColumnType
    nullable: bool = False

    @staticmethod
    def required(name: str, ty: ColumnType) -> ColumnDef:
        return ColumnDef(name=name, ty=ty, nullable=False)

    @staticmethod
    def optional(name: str, ty: ColumnType) -> ColumnDef:
        return ColumnDef(name=name, ty=ty, nullable=True)


@dataclass
class TableSchema:
    name: str
    columns: list[ColumnDef] = field(default_factory=list)
    primary_key: list[str] = field(default_factory=list)
    write_mode: WriteMode = WriteMode.APPEND


@dataclass
class StoreWrite:
    mode: WriteMode | None = None
    rows: list[StoreRow] = field(default_factory=list)

    @staticmethod
    def append(*rows: StoreRow) -> StoreWrite:
        return StoreWrite(mode=WriteMode.APPEND, rows=list(rows))


def store_row(**kwargs: StoreValue) -> StoreRow:
    return dict(kwargs)


@dataclass
class QualifiedTable:
    physical_name: str
    module: str
    logical_name: str
    schema: TableSchema
    mode: StorageMode


def with_identity_columns(schema: TableSchema) -> TableSchema:
    cols = [
        ColumnDef.required("_run_id", ColumnType.TEXT),
        ColumnDef.required("_work_id", ColumnType.TEXT),
        ColumnDef.required("_node_id", ColumnType.TEXT),
        ColumnDef.required("_module", ColumnType.TEXT),
    ]
    names = {c.name for c in cols}
    for c in schema.columns:
        if c.name not in names:
            cols.append(c)
    return TableSchema(
        name=schema.name,
        columns=cols,
        primary_key=list(schema.primary_key),
        write_mode=schema.write_mode,
    )


def qualify_table(
    mode: StorageMode, run_id: str, module: str, schema: TableSchema
) -> QualifiedTable:
    enriched = with_identity_columns(schema)
    if mode == StorageMode.PER_RUN:
        physical = f"{run_id}__{module}__{schema.name}"
    elif mode == StorageMode.SHARED:
        physical = f"{module}__{schema.name}"
    else:
        physical = schema.name
    return QualifiedTable(
        physical_name=physical,
        module=module,
        logical_name=schema.name,
        schema=enriched,
        mode=mode,
    )


def step_log_qualified(mode: StorageMode, run_id: str) -> QualifiedTable:
    schema = TableSchema(
        name=STEP_LOG_TABLE,
        columns=[
            ColumnDef.required("run_id", ColumnType.TEXT),
            ColumnDef.required("work_id", ColumnType.TEXT),
            ColumnDef.required("node_id", ColumnType.TEXT),
            ColumnDef.required("module", ColumnType.TEXT),
            ColumnDef.required("capability", ColumnType.TEXT),
            ColumnDef.required("ok", ColumnType.BOOLEAN),
            ColumnDef.optional("error", ColumnType.TEXT),
            ColumnDef.optional("output_ports", ColumnType.JSON),
        ],
        write_mode=WriteMode.APPEND,
    )
    physical = STEP_LOG_TABLE
    if mode == StorageMode.PER_RUN:
        physical = f"{run_id}__{STEP_LOG_TABLE}"
    qm = StorageMode.SHARED if mode == StorageMode.OFF else mode
    return QualifiedTable(
        physical_name=physical,
        module="",
        logical_name=STEP_LOG_TABLE,
        schema=schema,
        mode=qm,
    )


def inject_identity(
    row: StoreRow, run_id: str, work_id: str, node_id: str, module: str
) -> None:
    row.setdefault("_run_id", run_id)
    row.setdefault("_work_id", work_id)
    row.setdefault("_node_id", node_id)
    row.setdefault("_module", module)


class StorageBackend(Protocol):
    def ensure_table(self, table: QualifiedTable) -> None: ...
    def write_rows(
        self,
        table: str,
        mode: WriteMode,
        rows: list[StoreRow],
        primary_key: list[str],
        run_id: str,
    ) -> None: ...
    def drop_table(self, table: str) -> None: ...
    def table_exists(self, table: str) -> bool: ...
    def rows(self, table: str) -> list[StoreRow] | None: ...
    def table_names(self) -> list[str]: ...


class MemoryStorage:
    def __init__(self) -> None:
        self._tables: dict[str, dict[str, Any]] = {}

    def ensure_table(self, table: QualifiedTable) -> None:
        if table.physical_name in self._tables:
            existing = self._tables[table.physical_name]["schema"]
            if existing.columns != table.schema.columns:
                raise invalid(f"storage schema mismatch for table {table.physical_name}")
            return
        self._tables[table.physical_name] = {
            "schema": table.schema,
            "mode": table.mode,
            "rows": [],
        }

    def write_rows(
        self,
        table: str,
        mode: WriteMode,
        rows: list[StoreRow],
        primary_key: list[str],
        run_id: str,
    ) -> None:
        t = self._tables.get(table)
        if t is None:
            raise invalid(f"storage table not ensured: {table}")
        if mode in (WriteMode.APPEND,):
            t["rows"].extend(dict(r) for r in rows)
        elif mode == WriteMode.REPLACE:
            if t["mode"] == StorageMode.SHARED:
                t["rows"] = [
                    r
                    for r in t["rows"]
                    if r.get("_run_id", r.get("run_id")) != run_id
                ]
            else:
                t["rows"] = []
            t["rows"].extend(dict(r) for r in rows)
        elif mode == WriteMode.UPSERT:
            if not primary_key:
                raise invalid(f"upsert on {table} requires primary_key on TableSchema")
            for row in rows:
                idx = None
                for i, existing in enumerate(t["rows"]):
                    if all(existing.get(k) == row.get(k) for k in primary_key):
                        idx = i
                        break
                if idx is not None:
                    t["rows"][idx] = dict(row)
                else:
                    t["rows"].append(dict(row))
        else:
            raise invalid(f"unknown write mode {mode}")

    def drop_table(self, table: str) -> None:
        self._tables.pop(table, None)

    def table_exists(self, table: str) -> bool:
        return table in self._tables

    def rows(self, table: str) -> list[StoreRow] | None:
        t = self._tables.get(table)
        return None if t is None else [dict(r) for r in t["rows"]]

    def table_names(self) -> list[str]:
        return sorted(self._tables)
