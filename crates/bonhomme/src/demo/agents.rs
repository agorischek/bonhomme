use super::{
    AgentSliceRequest, AgentSliceResponse, ApplyAgentSliceRequest, DEMO_REPOSITORY, DemoMethod,
    DemoState, SpawnAgentsRequest, demo_state, display_name_method_id, ensure_demo,
    list_orders_method_id, order_service_class_id, stable_uuid,
};
use anyhow::{Result, bail};
use bonhomme_core::{MergeOutcome, Operation, Repository};
use bonhomme_engine::Storage;
use serde_json::json;
use std::collections::BTreeSet;

pub async fn spawn_agents(storage: &Storage, request: SpawnAgentsRequest) -> Result<DemoState> {
    ensure_demo(storage).await?;
    let repository = storage.repository_by_name(DEMO_REPOSITORY).await?;
    let existing = storage.list_branches(repository.id).await?;
    let existing_agent_numbers = existing
        .iter()
        .filter_map(|branch| branch.name.strip_prefix("agent-"))
        .filter_map(|suffix| suffix.parse::<usize>().ok())
        .collect::<BTreeSet<_>>();
    let start = existing_agent_numbers
        .iter()
        .next_back()
        .copied()
        .unwrap_or(0)
        + 1;

    for number in start..start + request.count {
        spawn_one_agent(storage, &repository, number, request.include_conflicts).await?;
    }

    demo_state(storage).await
}

pub async fn create_agent_slice(
    storage: &Storage,
    request: AgentSliceRequest,
) -> Result<AgentSliceResponse> {
    ensure_demo(storage).await?;
    let repository = storage.repository_by_name(DEMO_REPOSITORY).await?;
    if request.replace_branch.unwrap_or(false) {
        if request.branch_name == "main" {
            bail!("cannot replace the demo main branch");
        }
        storage
            .delete_branch_by_name(repository.id, &request.branch_name)
            .await?;
    }
    let branch = storage
        .create_branch(repository.id, &request.branch_name, "main")
        .await?;
    let materialized = storage
        .materialize_branch(DEMO_REPOSITORY, &request.branch_name)
        .await?;
    let root_symbols = if let Some(symbol) = request.symbol {
        materialized
            .graph
            .find_symbol(&symbol)
            .into_iter()
            .map(|symbol| symbol.id)
            .collect()
    } else {
        Vec::new()
    };
    let base_position = materialized.operations.len() as i64;
    let stored_slice = storage
        .create_slice(repository.id, branch.id, base_position, &root_symbols)
        .await?;
    let mut slice = storage.plugin().render_slice(
        &materialized.graph,
        format!("{}@{}", branch.name, base_position),
        root_symbols,
    );
    slice.id = stored_slice.id;
    Ok(AgentSliceResponse { branch, slice })
}

pub async fn apply_agent_slice(
    storage: &Storage,
    request: ApplyAgentSliceRequest,
) -> Result<DemoState> {
    ensure_demo(storage).await?;
    let repository = storage.repository_by_name(DEMO_REPOSITORY).await?;
    let slice_id = request.slice.id;
    let stored_slice = storage.slice_by_id(slice_id).await?;
    if stored_slice.repository_id != repository.id {
        bail!("slice {slice_id} does not belong to repository {DEMO_REPOSITORY}");
    }

    let branch = storage.branch_by_id(stored_slice.branch_id).await?;
    let branch_position_at_apply = storage
        .collect_branch_operations(branch.id, None)
        .await?
        .len() as i64;
    let materialized = storage
        .materialize_branch_at_position(branch.id, stored_slice.base_position)
        .await?;
    let operations = storage.plugin().recover_operations(
        &materialized.graph,
        &stored_slice.root_symbols,
        &request.slice.files,
    )?;
    let operations = bonhomme_core::detect_moves(operations, &materialized.graph);
    let analysis = storage
        .analyze_operations_against_branch(branch.id, stored_slice.base_position, &operations)
        .await?;
    if analysis.outcome == MergeOutcome::Conflict {
        storage
            .create_slice_apply_conflict(
                slice_id,
                &branch,
                bonhomme_engine::ConflictPositions::new(
                    stored_slice.base_position,
                    branch_position_at_apply,
                ),
                analysis.conflicts,
                operations,
                &request.agent,
            )
            .await?;
        return demo_state(storage).await;
    }

    let task = storage.create_task(repository.id, &request.title).await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            branch.id,
            &request.title,
            &request.agent,
        )
        .await?;
    let appended = storage
        .append_operations(repository.id, branch.id, changeset.id, operations.clone())
        .await?;
    storage
        .add_attachment(
            repository.id,
            "changeset",
            changeset.id,
            "SliceRecoveryAttachment",
            json!({
                "strategy": "demo-api-stored-slice-recovery",
                "sliceId": slice_id,
                "branch": branch.name,
                "basePosition": stored_slice.base_position,
                "branchPositionAtApply": branch_position_at_apply,
                "stale": branch_position_at_apply != stored_slice.base_position,
                "rootSymbols": stored_slice.root_symbols,
                "operationCount": operations.len(),
                "appendedOperationIds": appended.iter().map(|record| record.id).collect::<Vec<_>>(),
            }),
        )
        .await?;

    demo_state(storage).await
}

async fn spawn_one_agent(
    storage: &Storage,
    repository: &Repository,
    number: usize,
    include_conflicts: bool,
) -> Result<()> {
    let agent_name = format!("agent-{number:03}");
    let conflict_slot = include_conflicts && number.is_multiple_of(11);
    let demo_method = demo_method(number, conflict_slot, &agent_name);
    let branch = storage
        .create_branch(repository.id, &agent_name, "main")
        .await?;
    let task = storage
        .create_task(
            repository.id,
            &format!("{agent_name}: add {} to OrderService", demo_method.label),
        )
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            branch.id,
            &format!("{agent_name} {}", demo_method.label),
            &agent_name,
        )
        .await?;
    storage
        .add_attachment(
            repository.id,
            "task",
            task.id,
            "PromptAttachment",
            json!({
                "model": format!("agent-sim-{number:03}"),
                "prompt": format!("Add the {} capability to OrderService through a semantic slice.", demo_method.label)
            }),
        )
        .await?;

    let method_name = demo_method.name.clone();
    let method_id = stable_uuid(&format!("symbol/OrderService/{agent_name}/{method_name}"));
    let display_reference_id = stable_uuid(&format!(
        "reference/OrderService/{agent_name}/{method_name}/displayName"
    ));
    let list_reference_id = stable_uuid(&format!(
        "reference/OrderService/{agent_name}/{method_name}/listOrders"
    ));

    storage
        .append_operation(
            repository.id,
            branch.id,
            changeset.id,
            Operation::CreateSymbol {
                symbol_id: method_id,
                parent_id: Some(order_service_class_id()),
                kind: "method".to_string(),
                name: method_name.clone(),
                body: Some(demo_method.body),
                metadata: json!({"signature": format!("{method_name}(): string")}),
            },
        )
        .await?;
    storage
        .append_operation(
            repository.id,
            branch.id,
            changeset.id,
            Operation::CreateReference {
                reference_id: display_reference_id,
                from_symbol_id: method_id,
                to_symbol_id: display_name_method_id(),
                kind: "calls".to_string(),
            },
        )
        .await?;
    storage
        .append_operation(
            repository.id,
            branch.id,
            changeset.id,
            Operation::CreateReference {
                reference_id: list_reference_id,
                from_symbol_id: method_id,
                to_symbol_id: list_orders_method_id(),
                kind: "calls".to_string(),
            },
        )
        .await?;

    Ok(())
}

fn demo_method(number: usize, conflict_slot: bool, agent_name: &str) -> DemoMethod {
    if conflict_slot {
        return DemoMethod {
            name: "duplicateRiskReview".to_string(),
            label: "duplicate risk review".to_string(),
            body: format!(
                concat!(
                    "const stages = this.listOrders().join(\" / \");\n",
                    "return `${{this.displayName()}} risk review from {agent_name}: ${{stages}}`;"
                ),
                agent_name = agent_name
            ),
        };
    }

    let scenarios = [
        (
            "fulfillmentReadiness",
            "fulfillment readiness",
            "north dock",
        ),
        ("paymentRiskSignal", "payment risk signal", "payments"),
        ("inventoryReservation", "inventory reservation", "warehouse"),
        ("carrierRoutingPlan", "carrier routing plan", "last-mile"),
        ("returnWindowNotice", "return window notice", "returns"),
        (
            "loyaltyUpgradeHint",
            "loyalty upgrade hint",
            "customer care",
        ),
        ("taxCheckpoint", "tax checkpoint", "billing"),
        ("fraudEscalationNote", "fraud escalation note", "risk"),
        ("packingPriority", "packing priority", "packing"),
        ("backorderRecovery", "backorder recovery", "supply"),
        ("subscriptionHealth", "subscription health", "subscriptions"),
        ("invoiceNarrative", "invoice narrative", "finance"),
        ("warehouseLoadPlan", "warehouse load plan", "dock"),
        ("vipServicePromise", "VIP service promise", "concierge"),
        ("allocationHint", "allocation hint", "planner"),
        ("complianceStamp", "compliance stamp", "compliance"),
        ("deliveryPromise", "delivery promise", "routing"),
        ("refundReadiness", "refund readiness", "refunds"),
        ("batchPickingPlan", "batch picking plan", "picking"),
        ("serviceRecovery", "service recovery", "support"),
        ("giftWrapSignal", "gift wrap signal", "extras"),
        ("regionalCapacity", "regional capacity", "capacity"),
        ("priorityQueueNote", "priority queue note", "queueing"),
        ("handoffSummary", "handoff summary", "operations"),
    ];
    let (base_name, label, lane) = scenarios[(number - 1) % scenarios.len()];
    let cycle = ((number - 1) / scenarios.len()) + 1;
    let name = if cycle == 1 {
        base_name.to_string()
    } else {
        format!("{base_name}{cycle}")
    };
    let promise_hours = 12 + (number % 6) * 6;
    let signal = ["green", "amber", "blue", "silver", "gold"][number % 5];

    DemoMethod {
        name,
        label: label.to_string(),
        body: format!(
            concat!(
                "const stages = this.listOrders().join(\" -> \");\n",
                "return `${{this.displayName()}} {label} on {lane} lane: ",
                "{signal} within {promise_hours}h (${{stages}})`;"
            ),
            label = label,
            lane = lane,
            signal = signal,
            promise_hours = promise_hours
        ),
    }
}
