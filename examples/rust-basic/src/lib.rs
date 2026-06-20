pub struct OrderService {
    service_name: String,
}

impl OrderService {
    pub fn display_name(&self) -> &str {
        &self.service_name
    }

    pub fn list_orders(&self) -> Vec<&'static str> {
        vec!["intake", "packing", "shipped"]
    }

    pub fn summary(&self) -> String {
        format_order(self.display_name())
    }
}

pub fn format_order(id: &str) -> String {
    format!("order:{id}")
}
