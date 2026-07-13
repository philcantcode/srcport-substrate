//! Optional storage phase: schema at register, write after step, retention modes.

use srcport_framework::{
    store_row, ColumnDef, ColumnType, FrameworkError, FrameworkPolicy, Host, MemoryStorage,
    ModulePlugin, PortBody, StepContext, StepOutput, StepResult, StoragePlan, StorageRetention,
    StoreValue, StoreWrite, TableSchema, WriteMode, STEP_LOG_TABLE,
};
use srcport_substrate::{artifact_with_trait, has_traits, 
    Artifact, Assembly, AssemblyNode, Binding, Capability, MemoryKernel, ModuleManifest,
    NamedArtifact, NodeOutput, Port, RunState,
};

struct Counter;

impl ModulePlugin for Counter {
    fn manifest(&self) -> ModuleManifest {
        ModuleManifest {
            name: "counter".into(),
            version: "1.0.0".into(),
            provides: vec![Capability {
                name: "count.run".into(),
                inputs: vec![Port {
                    name: "seed".into(),
                    traits: vec!["demo.v1.Seed".into()],
                    ..Default::default()
                }],
                outputs: vec![Port {
                    name: "total".into(),
                    traits: vec!["demo.v1.Total".into()],
                    ..Default::default()
                }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn storage_schema(&self) -> Option<TableSchema> {
        Some(
            TableSchema::new("counts")
                .column(ColumnDef::required("key", ColumnType::Text))
                .column(ColumnDef::required("n", ColumnType::Integer))
                .primary_key(["key"])
                .write_mode(WriteMode::Append),
        )
    }

    fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
        let body = step
            .inputs
            .get("seed")
            .and_then(|a| a.traits.values().next().map(|f| f.body.clone()))
            .unwrap_or_else(|| b"1".to_vec());
        Ok(StepOutput {
            outputs: vec![PortBody::with_trait("total", "demo.v1.Total", body)],
        })
    }

    fn on_store(&self, step: &StepContext, result: &StepResult) -> Option<StoreWrite> {
        if !result.ok {
            return None;
        }
        let n: i64 = step
            .inputs
            .get("seed")
            .and_then(|a| std::str::from_utf8(a.traits.values().next().map(|f| f.body.as_slice()).unwrap_or(b"")).ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Some(StoreWrite::append([store_row([
            ("key", StoreValue::Text("seed".into())),
            ("n", StoreValue::Integer(n)),
        ])]))
    }
}

fn put_seed(host: &Host<MemoryKernel>, body: &[u8]) -> NamedArtifact {
    let r = host
        .kernel()
        .put_artifact({ let mut __a = artifact_with_trait("demo.v1.Seed", body.to_vec()); __a.produced_by = "test".into(); __a })
        .unwrap();
    NamedArtifact {
        name: "seed".into(),
        artifact: Some(r),
    }
}

fn assembly() -> Assembly {
    Assembly {
        id: "counter-pipe".into(),
        nodes: vec![AssemblyNode {
            id: "n1".into(),
            module: "counter".into(),
            module_version: "1.0.0".into(),
            capability: "count.run".into(),
        }],
        bindings: vec![Binding {
            to_node: "n1".into(),
            to_port: "seed".into(),
            input: "seed".into(),
            ..Default::default()
        }],
        terminal: Some(NodeOutput {
            node: "n1".into(),
            port: "total".into(),
        }),
    }
}

#[test]
fn per_run_registers_table_and_writes_after_step() {
    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Counter)).unwrap();

    let seed = put_seed(&host, b"42");
    let run = host
        .start_pipeline(
            "run-a",
            assembly(),
            vec![seed],
            FrameworkPolicy::converge().with_storage(StoragePlan::per_run_keep()),
        )
        .unwrap();
    assert_eq!(run.state(), RunState::Running);

    let table = "run-a__counter__counts";
    assert!(host.storage().unwrap().table_exists(table));

    let run = host.drive("run-a").unwrap();
    assert_eq!(run.state(), RunState::Completed);

    // Keep retention: table still present after complete.
    let rows = host.storage().unwrap().rows(table).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("n"), Some(&StoreValue::Integer(42)));
    assert_eq!(
        rows[0].get("_run_id"),
        Some(&StoreValue::Text("run-a".into()))
    );
    assert_eq!(
        rows[0].get("_module"),
        Some(&StoreValue::Text("counter".into()))
    );
}

#[test]
fn per_run_drop_on_end_removes_tables() {
    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Counter)).unwrap();

    let seed = put_seed(&host, b"7");
    host.start_pipeline(
        "run-drop",
        assembly(),
        vec![seed],
        FrameworkPolicy::converge().with_storage(StoragePlan::per_run()), // DropOnEnd
    )
    .unwrap();

    let table = "run-drop__counter__counts";
    assert!(host.storage().unwrap().table_exists(table));

    host.drive("run-drop").unwrap();
    assert!(!host.storage().unwrap().table_exists(table));
}

#[test]
fn shared_mode_table_survives_across_runs() {
    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Counter)).unwrap();

    let table = "counter__counts";

    for (id, body) in [("r1", b"1" as &[u8]), ("r2", b"2")] {
        let seed = put_seed(&host, body);
        host.start_pipeline(
            id,
            assembly(),
            vec![seed],
            FrameworkPolicy::converge().with_storage(StoragePlan::shared()),
        )
        .unwrap();
        host.drive(id).unwrap();
    }

    assert!(host.storage().unwrap().table_exists(table));
    let rows = host.storage().unwrap().rows(table).unwrap();
    assert_eq!(rows.len(), 2);
    let run_ids: Vec<_> = rows
        .iter()
        .filter_map(|r| match r.get("_run_id") {
            Some(StoreValue::Text(s)) => Some(s.as_str()),
            _ => None,
        })
        .collect();
    assert!(run_ids.contains(&"r1"));
    assert!(run_ids.contains(&"r2"));
}

#[test]
fn step_log_only_writes_audit_rows() {
    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Counter)).unwrap();

    let seed = put_seed(&host, b"9");
    host.start_pipeline(
        "log-run",
        assembly(),
        vec![seed],
        FrameworkPolicy::converge().with_storage(StoragePlan::step_log_only()),
    )
    .unwrap();
    host.drive("log-run").unwrap();

    // No module table in step_log_only.
    assert!(!host.storage().unwrap().table_exists("log-run__counter__counts"));
    assert!(!host.storage().unwrap().table_exists("counter__counts"));

    let rows = host.storage().unwrap().rows(STEP_LOG_TABLE).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("module"),
        Some(&StoreValue::Text("counter".into()))
    );
    assert_eq!(rows[0].get("ok"), Some(&StoreValue::Boolean(true)));
}

#[test]
fn storage_plan_without_backend_errors() {
    let mut host = Host::new(MemoryKernel::new());
    host.register_plugin(Box::new(Counter)).unwrap();
    let seed = put_seed(&host, b"1");
    let err = host
        .start_pipeline(
            "no-backend",
            assembly(),
            vec![seed],
            FrameworkPolicy::converge().with_storage(StoragePlan::per_run()),
        )
        .unwrap_err();
    assert!(matches!(err, FrameworkError::Invalid(_)));
}

#[test]
fn upsert_write_mode_across_shared_runs() {
    struct Upserter;
    impl ModulePlugin for Upserter {
        fn manifest(&self) -> ModuleManifest {
            ModuleManifest {
                name: "up".into(),
                version: "1.0.0".into(),
                provides: vec![Capability {
                    name: "up.run".into(),
                    inputs: vec![Port {
                        name: "in".into(),
                        traits: vec!["demo.v1.In".into()],
                        ..Default::default()
                    }],
                    outputs: vec![Port {
                        name: "out".into(),
                        traits: vec!["demo.v1.Out".into()],
                        ..Default::default()
                    }],
                    ..Default::default()
                }],
                ..Default::default()
            }
        }

        fn storage_schema(&self) -> Option<TableSchema> {
            Some(
                TableSchema::new("state")
                    .column(ColumnDef::required("k", ColumnType::Text))
                    .column(ColumnDef::required("v", ColumnType::Text))
                    .primary_key(["k"])
                    .write_mode(WriteMode::Upsert),
            )
        }

        fn execute(&mut self, step: &mut StepContext) -> Result<StepOutput, FrameworkError> {
            let body = step
                .inputs
                .get("in")
                .and_then(|a| a.traits.values().next().map(|f| f.body.clone()))
                .unwrap_or_default();
            Ok(StepOutput {
                outputs: vec![PortBody::with_trait("out", "demo.v1.Out", body)],
            })
        }

        fn on_store(&self, step: &StepContext, result: &StepResult) -> Option<StoreWrite> {
            if !result.ok {
                return None;
            }
            let v = step
                .inputs
                .get("in")
                .and_then(|a| String::from_utf8(a.traits.values().next().map(|f| f.body.clone()).unwrap_or_default()).ok())
                .unwrap_or_default();
            Some(StoreWrite::upsert([store_row([
                ("k", StoreValue::Text("only".into())),
                ("v", StoreValue::Text(v)),
            ])]))
        }
    }

    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Upserter)).unwrap();

    let asm = Assembly {
        id: "up-pipe".into(),
        nodes: vec![AssemblyNode {
            id: "n".into(),
            module: "up".into(),
            module_version: "1.0.0".into(),
            capability: "up.run".into(),
        }],
        bindings: vec![Binding {
            to_node: "n".into(),
            to_port: "in".into(),
            input: "in".into(),
            ..Default::default()
        }],
        terminal: Some(NodeOutput {
            node: "n".into(),
            port: "out".into(),
        }),
    };

    for (id, val) in [("u1", "first"), ("u2", "second")] {
        let r = host
            .kernel()
            .put_artifact({ let mut __a = artifact_with_trait("demo.v1.In", val.as_bytes().to_vec()); __a.produced_by = "test".into(); __a })
            .unwrap();
        host.start_pipeline(
            id,
            asm.clone(),
            vec![NamedArtifact {
                name: "in".into(),
                artifact: Some(r),
            }],
            FrameworkPolicy::converge().with_storage(StoragePlan::shared()),
        )
        .unwrap();
        host.drive(id).unwrap();
    }

    let rows = host.storage().unwrap().rows("up__state").unwrap();
    // Upsert by k=only → still one row, last write wins.
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get("v"), Some(&StoreValue::Text("second".into())));
}

#[test]
fn storage_off_by_default() {
    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Counter)).unwrap();
    let seed = put_seed(&host, b"1");
    host.start_pipeline(
        "plain",
        assembly(),
        vec![seed],
        FrameworkPolicy::converge(), // storage off
    )
    .unwrap();
    host.drive("plain").unwrap();
    assert!(host.storage().unwrap().table_names().is_empty());
}

#[test]
fn cancel_drops_per_run_tables() {
    let mut host = Host::new(MemoryKernel::new()).with_storage(MemoryStorage::new());
    host.register_plugin(Box::new(Counter)).unwrap();
    let seed = put_seed(&host, b"1");
    host.start_pipeline(
        "c1",
        assembly(),
        vec![seed],
        FrameworkPolicy::converge().with_storage(
            StoragePlan::per_run().with_retention(StorageRetention::DropOnEnd),
        ),
    )
    .unwrap();
    let table = "c1__counter__counts";
    assert!(host.storage().unwrap().table_exists(table));
    host.cancel("c1").unwrap();
    assert!(!host.storage().unwrap().table_exists(table));
}
