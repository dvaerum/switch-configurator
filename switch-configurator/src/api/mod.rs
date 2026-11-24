pub mod server;
pub mod handlers;
pub mod poe_batch;
pub mod prefix;

#[cfg(test)]
mod tests;

pub use server::{create_router, API_ENDPOINTS};
