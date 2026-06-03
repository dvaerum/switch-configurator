pub mod server;
pub mod handlers;

#[cfg(test)]
mod tests;

pub use server::{create_router, API_ENDPOINTS};
