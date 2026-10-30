//! Catches scam messages and anyone posting in the honeypot channel.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "guard";

#[derive(Default)]
pub struct Guard;

#[feature_async]
impl Feature for Guard {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Scam protection",
            description: "Catches scam messages and anyone posting in the honeypot channel.",
            group: Group::Safety,
            nav: vec![NavItem { label: "Scam protection", path: KEY, icon: Icon::Shield }],
        }
    }
}
