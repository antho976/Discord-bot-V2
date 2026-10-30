//! AFK status and member timezones.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "utility";

#[derive(Default)]
pub struct Utility;

#[feature_async]
impl Feature for Utility {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "AFK and timezones",
            description: "AFK status and member timezones.",
            group: Group::Tools,
            nav: vec![NavItem { label: "AFK and timezones", path: KEY, icon: Icon::Clock }],
        }
    }
}
