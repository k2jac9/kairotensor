#[path = "incident_store_bridge/adapter.rs"]
mod adapter;
#[cfg(test)]
#[path = "incident_store_bridge/tests.rs"]
mod tests;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    adapter::demo()
}
