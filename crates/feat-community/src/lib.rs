//! Giveaways, polls and suggestions.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "community";

#[derive(Default)]
pub struct Community;

#[feature_async]
impl Feature for Community {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Community",
            description: "Giveaways, polls and suggestions.",
            group: Group::Community,
            nav: vec![NavItem { label: "Community", path: KEY, icon: Icon::Gift }],
        }
    }
}
