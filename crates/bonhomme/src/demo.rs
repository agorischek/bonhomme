mod agents;
mod ids;
mod merge;
mod seed;
mod state;
mod types;

pub use agents::spawn_agents;
pub use ids::{
    display_name_method_id, list_orders_method_id, order_service_class_id, order_service_file_id,
    stable_uuid,
};
pub use merge::{merge_all_agents, merge_next_agent};
pub use seed::{ensure_demo, reset_demo};
pub use state::demo_state;
pub use types::{
    BranchStatus, BranchSummary, DemoMergeRun, DemoMetrics, DemoState, OperationView,
    SpawnAgentsRequest,
};

use types::DemoMethod;

pub const DEMO_REPOSITORY: &str = "bonhomme-demo";
