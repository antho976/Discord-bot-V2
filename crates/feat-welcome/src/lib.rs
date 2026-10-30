//! Welcome and goodbye messages with a card, auto-roles, roles back on rejoin, invite tracking.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "welcome";

#[derive(Default)]
pub struct Welcome;

#[feature_async]
impl Feature for Welcome {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Welcome",
            description: "Welcome and goodbye messages with a card, auto-roles, roles back on rejoin, invite tracking.",
            group: Group::Members,
            nav: vec![NavItem { label: "Welcome", path: KEY, icon: Icon::Wave }],
        }
    }
}
