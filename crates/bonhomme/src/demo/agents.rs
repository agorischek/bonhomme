use super::{
    DEMO_REPOSITORY, DemoMethod, DemoState, SpawnAgentsRequest, demo_state, display_name_method_id,
    ensure_demo, list_orders_method_id, order_service_class_id, stable_uuid,
};
use anyhow::Result;
use bonhomme_core::{Operation, Repository};
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
