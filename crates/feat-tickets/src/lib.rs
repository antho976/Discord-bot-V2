//! Support tickets as private threads, and a starboard for popular messages.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "tickets";

#[derive(Default)]
pub struct Tickets;

#[feature_async]
impl Feature for Tickets {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Tickets and starboard",
            description: "Support tickets as private threads, and a starboard for popular messages.",
            group: Group::Community,
            nav: vec![NavItem { label: "Tickets and starboard", path: KEY, icon: Icon::Ticket }],
        }
    }
}
