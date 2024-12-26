pub mod fire_and_forget;
pub mod self_updating_task;
pub mod variable_task;
pub mod task;
mod common;

pub use fire_and_forget::fire_and_forget_polling_task;
pub use fire_and_forget::self_updating_fire_and_forget_polling_task;
pub use fire_and_forget::variable_fire_and_forget_polling_task;

pub use self_updating_task::SelfUpdatingPollingTask;
pub use task::PollingTask;
pub use variable_task::VariablePollingTask;
