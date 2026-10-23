pub mod audit;
pub mod history;
pub mod model;
pub mod sms;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type Result<T> = std::result::Result<T, Error>;
