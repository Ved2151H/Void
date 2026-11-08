// Each module holds one command domain; shared.rs holds cross-domain helpers.
mod apps;
mod audio;
mod dock;
mod media;
mod network;
mod power;
mod settings;
mod shelf;
mod shared;
mod system_info;
mod tray;
mod windows;

pub use apps::*;
pub use audio::*;
pub use dock::*;
pub use media::*;
pub use network::*;
pub use power::*;
pub use settings::*;
pub use shelf::*;
pub(crate) use shared::*;
pub use system_info::*;
pub use tray::*;
pub use windows::*;
