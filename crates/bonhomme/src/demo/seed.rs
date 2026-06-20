use super::{
    DEMO_REPOSITORY, DemoState, demo_state, display_name_method_id, list_orders_method_id,
    order_service_class_id, order_service_file_id, stable_uuid,
};
use anyhow::Result;
use bonhomme_core::{Branch, Operation, Repository};
use bonhomme_engine::Storage;
use serde_json::json;

pub async fn reset_demo(storage: &Storage) -> Result<DemoState> {
    let (repository, main) = storage.reset_repository(DEMO_REPOSITORY).await?;
    seed_initial_order_service(storage, &repository, &main).await?;
    demo_state(storage).await
}

pub async fn ensure_demo(storage: &Storage) -> Result<DemoState> {
    match storage.repository_by_name(DEMO_REPOSITORY).await {
        Ok(_) => demo_state(storage).await,
        Err(_) => reset_demo(storage).await,
    }
}

async fn seed_initial_order_service(
    storage: &Storage,
    repository: &Repository,
    main: &Branch,
) -> Result<()> {
    let task = storage
        .create_task(repository.id, "Import TypeScript OrderService")
        .await?;
    let changeset = storage
        .create_changeset(
            repository.id,
            task.id,
            main.id,
            "Seed semantic graph from TypeScript",
            "importer",
        )
        .await?;

    storage
        .add_attachment(
            repository.id,
            "task",
            task.id,
            "PromptAttachment",
            json!({
                "model": "human",
                "prompt": "Initialize the bonhomme demo repository with a TypeScript OrderService."
            }),
        )
        .await?;

    let file_id = order_service_file_id();
    let class_id = order_service_class_id();
    let display_name_id = display_name_method_id();
    let service_name_id = stable_uuid("symbol/OrderService/serviceName");
    let default_region_id = stable_uuid("symbol/OrderService/defaultRegion");
    let list_orders_id = list_orders_method_id();

    let operations = vec![
        Operation::CreateSymbol {
            symbol_id: file_id,
            parent_id: None,
            kind: "file".to_string(),
            name: "OrderService.ts".to_string(),
            body: None,
            metadata: json!({"path": "src/OrderService.ts"}),
        },
        Operation::CreateSymbol {
            symbol_id: class_id,
            parent_id: Some(file_id),
            kind: "class".to_string(),
            name: "OrderService".to_string(),
            body: None,
            metadata: json!({"exported": true}),
        },
        Operation::CreateSymbol {
            symbol_id: service_name_id,
            parent_id: Some(class_id),
            kind: "property".to_string(),
            name: "serviceName".to_string(),
            body: None,
            metadata: json!({"declaration": "private readonly serviceName = \"OrderService\";"}),
        },
        Operation::CreateSymbol {
            symbol_id: default_region_id,
            parent_id: Some(class_id),
            kind: "property".to_string(),
            name: "defaultRegion".to_string(),
            body: None,
            metadata: json!({"declaration": "private readonly defaultRegion = \"north-america\";"}),
        },
        Operation::CreateSymbol {
            symbol_id: display_name_id,
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "displayName".to_string(),
            body: Some("return this.serviceName;".to_string()),
            metadata: json!({"signature": "displayName(): string"}),
        },
        Operation::CreateSymbol {
            symbol_id: list_orders_id,
            parent_id: Some(class_id),
            kind: "method".to_string(),
            name: "listOrders".to_string(),
            body: Some(
                "return [\"intake\", \"payment\", \"picking\", \"packing\", \"shipped\"];"
                    .to_string(),
            ),
            metadata: json!({"signature": "listOrders(): string[]"}),
        },
    ];

    for operation in operations {
        storage
            .append_operation(repository.id, main.id, changeset.id, operation)
            .await?;
    }

    Ok(())
}
