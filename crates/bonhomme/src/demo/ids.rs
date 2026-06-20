use uuid::Uuid;

pub fn stable_uuid(label: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("https://bonhomme.local/{label}").as_bytes(),
    )
}

pub fn order_service_file_id() -> Uuid {
    stable_uuid("symbol/src/OrderService.ts")
}

pub fn order_service_class_id() -> Uuid {
    stable_uuid("symbol/OrderService")
}

pub fn display_name_method_id() -> Uuid {
    stable_uuid("symbol/OrderService/displayName")
}

pub fn list_orders_method_id() -> Uuid {
    stable_uuid("symbol/OrderService/listOrders")
}
