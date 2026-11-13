pub mod server;
pub mod handlers;
pub mod prefix;

#[cfg(test)]
mod tests;

pub use server::{create_router, API_ENDPOINTS};
