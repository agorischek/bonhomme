use super::*;
use chrono::{TimeZone, Utc};
use proptest::prelude::*;
use serde_json::json;
use uuid::Uuid;

fn record(position: i64, operation: Operation) -> OperationRecord {
    OperationRecord {
        id: Uuid::new_v4(),
        repository_id: Uuid::nil(),
        branch_id: Uuid::nil(),
        changeset_id: Uuid::nil(),
        position,
        operation,
        created_at: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

fn stable_uuid(label: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("bonhomme-core-test/{label}").as_bytes(),
    )
}

fn stable_record(label: &str, position: i64, operation: Operation) -> OperationRecord {
    OperationRecord {
        id: stable_uuid(&format!("operation/{label}")),
        repository_id: Uuid::nil(),
        branch_id: stable_uuid(&format!("branch/{label}")),
        changeset_id: stable_uuid(&format!("changeset/{label}")),
        position,
        operation,
        created_at: Utc.timestamp_opt(0, 0).unwrap(),
    }
}

#[test]
fn independent_method_additions_do_not_conflict() {
    let class_id = Uuid::new_v4();
    let source = vec![record(
        1,
        Operation::CreateSymbol {
            symbol_id: Uuid::new_v4(),
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "agentOne".to_string(),
            body: Some("return 1;".to_string()),
            metadata: json!({"signature": "agentOne(): number"}),
        },
    )];
    let target = vec![record(
        1,
        Operation::CreateSymbol {
            symbol_id: Uuid::new_v4(),
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "agentTwo".to_string(),
            body: Some("return 2;".to_string()),
            metadata: json!({"signature": "agentTwo(): number"}),
        },
    )];

    let analysis = analyze_merge(&target, &source);

    assert_eq!(analysis.outcome, MergeOutcome::SafeMerge);
    assert!(analysis.conflicts.is_empty());
}

#[test]
fn duplicate_method_name_conflicts() {
    let class_id = Uuid::new_v4();
    let source = vec![record(
        1,
        Operation::CreateSymbol {
            symbol_id: Uuid::new_v4(),
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "audit".to_string(),
            body: Some("return true;".to_string()),
            metadata: json!({"signature": "audit(): boolean"}),
        },
    )];
    let target = vec![record(
        1,
        Operation::CreateSymbol {
            symbol_id: Uuid::new_v4(),
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "audit".to_string(),
            body: Some("return false;".to_string()),
            metadata: json!({"signature": "audit(): boolean"}),
        },
    )];

    let analysis = analyze_merge(&target, &source);

    assert_eq!(analysis.outcome, MergeOutcome::Conflict);
    assert_eq!(analysis.conflicts[0].reason, "DUPLICATE_SYMBOL_NAME");
}

#[test]
fn independent_references_to_same_target_do_not_conflict() {
    let display_name_id = Uuid::new_v4();
    let source_method_id = Uuid::new_v4();
    let target_method_id = Uuid::new_v4();
    let source = vec![record(
        1,
        Operation::CreateReference {
            reference_id: Uuid::new_v4(),
            from_symbol_id: source_method_id,
            to_symbol_id: display_name_id,
            kind: "calls".to_string(),
        },
    )];
    let target = vec![record(
        1,
        Operation::CreateReference {
            reference_id: Uuid::new_v4(),
            from_symbol_id: target_method_id,
            to_symbol_id: display_name_id,
            kind: "calls".to_string(),
        },
    )];

    let analysis = analyze_merge(&target, &source);

    assert_eq!(analysis.outcome, MergeOutcome::SafeMerge);
}

#[test]
fn reference_to_symbol_deleted_by_target_conflicts() {
    let from_id = Uuid::new_v4();
    let to_id = Uuid::new_v4();
    let source = vec![record(
        1,
        Operation::CreateReference {
            reference_id: Uuid::new_v4(),
            from_symbol_id: from_id,
            to_symbol_id: to_id,
            kind: "calls".to_string(),
        },
    )];
    let target = vec![record(1, Operation::DeleteSymbol { symbol_id: to_id })];

    let analysis = analyze_merge(&target, &source);

    assert_eq!(analysis.outcome, MergeOutcome::Conflict);
    assert!(
        analysis
            .conflicts
            .iter()
            .any(|conflict| conflict.reason == "REFERENCE_TO_DELETED_SYMBOL")
    );
}

#[test]
fn concurrent_reference_deletes_conflict() {
    let reference_id = Uuid::new_v4();
    let source = vec![record(1, Operation::DeleteReference { reference_id })];
    let target = vec![record(1, Operation::DeleteReference { reference_id })];

    let analysis = analyze_merge(&target, &source);

    assert_eq!(analysis.outcome, MergeOutcome::Conflict);
    assert!(
        analysis
            .conflicts
            .iter()
            .any(|conflict| conflict.reason == "OVERLAPPING_REFERENCE_WRITE")
    );
}

#[test]
fn dependency_queries_follow_references() {
    let file_id = Uuid::new_v4();
    let class_id = Uuid::new_v4();
    let caller_id = Uuid::new_v4();
    let callee_id = Uuid::new_v4();
    let reference_id = Uuid::new_v4();
    let records = vec![
        record(
            1,
            Operation::CreateSymbol {
                symbol_id: file_id,
                parent_id: None,
                kind: "file".to_string(),
                name: "Service.ts".to_string(),
                body: None,
                metadata: json!({"path": "Service.ts"}),
            },
        ),
        record(
            2,
            Operation::CreateSymbol {
                symbol_id: class_id,
                parent_id: Some(file_id),
                kind: "class".to_string(),
                name: "Service".to_string(),
                body: None,
                metadata: json!({}),
            },
        ),
        record(
            3,
            Operation::CreateSymbol {
                symbol_id: caller_id,
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "caller".to_string(),
                body: Some("return this.callee();".to_string()),
                metadata: json!({"signature": "caller(): string"}),
            },
        ),
        record(
            4,
            Operation::CreateSymbol {
                symbol_id: callee_id,
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "callee".to_string(),
                body: Some("return \"ok\";".to_string()),
                metadata: json!({"signature": "callee(): string"}),
            },
        ),
        record(
            5,
            Operation::CreateReference {
                reference_id,
                from_symbol_id: caller_id,
                to_symbol_id: callee_id,
                kind: "calls".to_string(),
            },
        ),
    ];
    let graph = materialize(&records).unwrap();

    assert_eq!(graph.find_dependencies(caller_id)[0].id, callee_id);
    assert_eq!(graph.find_dependents(callee_id)[0].id, caller_id);
}

proptest! {
    #[test]
    fn unique_method_additions_commute_without_conflicts(
        names in prop::collection::btree_set("[a-z][a-z0-9]{0,7}", 2..32)
    ) {
        let class_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, b"bonhomme-test-class");
        let names = names.into_iter().collect::<Vec<_>>();
        let midpoint = names.len() / 2;
        let make_records = |slice: &[String]| {
            slice
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    record(
                        index as i64 + 1,
                        Operation::CreateSymbol {
                            symbol_id: Uuid::new_v5(
                                &Uuid::NAMESPACE_URL,
                                format!("bonhomme-test-method-{name}").as_bytes(),
                            ),
                            parent_id: Some(class_id),
                            kind: "method".to_string(),
                            name: name.clone(),
                            body: Some(format!("return \"{name}\";")),
                            metadata: json!({"signature": format!("{name}(): string")}),
                        },
                    )
                })
                .collect::<Vec<_>>()
        };
        let left = make_records(&names[..midpoint]);
        let right = make_records(&names[midpoint..]);

        prop_assert_eq!(analyze_merge(&left, &right).outcome, MergeOutcome::SafeMerge);
        prop_assert_eq!(analyze_merge(&right, &left).outcome, MergeOutcome::SafeMerge);
    }
}

#[test]
fn simulated_large_branch_sequence_replays_deterministically() {
    let agent_count = 512;
    let first = simulate_large_branch_sequence(agent_count);
    let second = simulate_large_branch_sequence(agent_count);

    assert_eq!(first, second);
    assert_eq!(first.symbols.len(), agent_count + 3);
    assert_eq!(first.references.len(), agent_count);
    assert_eq!(first.applied_operations.len(), (agent_count * 2) + 3);
}

fn simulate_large_branch_sequence(agent_count: usize) -> SemanticGraph {
    let base = base_records();
    let mut graph = materialize(&base).unwrap();
    let mut target_since_base = Vec::new();
    let mut order = (0..agent_count).collect::<Vec<_>>();
    order.sort_by_key(|number| stable_order_key(&format!("agent-{number:03}")));

    for number in order {
        let source = agent_records(number);
        let analysis = analyze_merge(&target_since_base, &source);
        assert_eq!(analysis.outcome, MergeOutcome::SafeMerge);
        for record in &source {
            graph.apply_record(record).unwrap();
        }
        target_since_base.extend(source);
    }

    graph.validate().unwrap();
    graph
}

fn base_records() -> Vec<OperationRecord> {
    let file_id = stable_uuid("symbol/file");
    let class_id = stable_uuid("symbol/OrderService");
    let display_id = stable_uuid("symbol/OrderService/displayName");

    vec![
        stable_record(
            "base-file",
            1,
            Operation::CreateSymbol {
                symbol_id: file_id,
                parent_id: None,
                kind: "file".to_string(),
                name: "OrderService.ts".to_string(),
                body: None,
                metadata: json!({"path": "OrderService.ts"}),
            },
        ),
        stable_record(
            "base-class",
            2,
            Operation::CreateSymbol {
                symbol_id: class_id,
                parent_id: Some(file_id),
                kind: "class".to_string(),
                name: "OrderService".to_string(),
                body: None,
                metadata: json!({"exported": true}),
            },
        ),
        stable_record(
            "base-display",
            3,
            Operation::CreateSymbol {
                symbol_id: display_id,
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: "displayName".to_string(),
                body: Some("return \"OrderService\";".to_string()),
                metadata: json!({"signature": "displayName(): string"}),
            },
        ),
    ]
}

fn agent_records(number: usize) -> Vec<OperationRecord> {
    let class_id = stable_uuid("symbol/OrderService");
    let display_id = stable_uuid("symbol/OrderService/displayName");
    let method_id = stable_uuid(&format!("symbol/OrderService/agent-{number:03}"));
    let reference_id = stable_uuid(&format!("reference/agent-{number:03}/displayName"));
    let method_name = format!("agent{number:03}Status");

    vec![
        stable_record(
            &format!("agent-{number:03}-method"),
            1,
            Operation::CreateSymbol {
                symbol_id: method_id,
                parent_id: Some(class_id),
                kind: "method".to_string(),
                name: method_name.clone(),
                body: Some(format!("return `${{this.displayName()}} {number}`;")),
                metadata: json!({"signature": format!("{method_name}(): string")}),
            },
        ),
        stable_record(
            &format!("agent-{number:03}-reference"),
            2,
            Operation::CreateReference {
                reference_id,
                from_symbol_id: method_id,
                to_symbol_id: display_id,
                kind: "calls".to_string(),
            },
        ),
    ]
}

fn stable_order_key(name: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in name.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

#[test]
fn move_symbol_reparents_preserving_identity() {
    let file = |id, name: &str| Operation::CreateSymbol {
        symbol_id: id,
        parent_id: None,
        kind: "file".to_string(),
        name: name.to_string(),
        body: None,
        metadata: json!({}),
    };
    let file_a = stable_uuid("file-a");
    let file_b = stable_uuid("file-b");
    let class = stable_uuid("class-c");

    let graph = materialize(&[
        record(1, file(file_a, "a.ts")),
        record(2, file(file_b, "b.ts")),
        record(
            3,
            Operation::CreateSymbol {
                symbol_id: class,
                parent_id: Some(file_a),
                kind: "class".to_string(),
                name: "C".to_string(),
                body: None,
                metadata: json!({}),
            },
        ),
        record(
            4,
            Operation::MoveSymbol {
                symbol_id: class,
                new_parent_id: Some(file_b),
            },
        ),
    ])
    .expect("identity-preserving move should replay cleanly");

    let moved = &graph.symbols[&class];
    assert_eq!(moved.parent_id, Some(file_b), "class now lives under file B");
    assert_eq!(moved.id, class, "id preserved across the move");
    assert_eq!(moved.name, "C", "name preserved across the move");
}

#[test]
fn move_symbol_rejects_cycle_and_missing_target() {
    let parent = stable_uuid("p");
    let child = stable_uuid("c");
    let base = || {
        vec![
            record(
                1,
                Operation::CreateSymbol {
                    symbol_id: parent,
                    parent_id: None,
                    kind: "file".to_string(),
                    name: "p.ts".to_string(),
                    body: None,
                    metadata: json!({}),
                },
            ),
            record(
                2,
                Operation::CreateSymbol {
                    symbol_id: child,
                    parent_id: Some(parent),
                    kind: "class".to_string(),
                    name: "C".to_string(),
                    body: None,
                    metadata: json!({}),
                },
            ),
        ]
    };

    // Moving a symbol beneath its own descendant would form a cycle.
    let mut cyclic = base();
    cyclic.push(record(
        3,
        Operation::MoveSymbol {
            symbol_id: parent,
            new_parent_id: Some(child),
        },
    ));
    assert!(
        materialize(&cyclic).is_err(),
        "cycle-forming move must be rejected"
    );

    // Moving a symbol that does not exist.
    let mut missing = base();
    missing.push(record(
        3,
        Operation::MoveSymbol {
            symbol_id: stable_uuid("ghost"),
            new_parent_id: None,
        },
    ));
    assert!(
        materialize(&missing).is_err(),
        "moving a missing symbol must be rejected"
    );
}
