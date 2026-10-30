//! Idleon companion collection, pet giveaways and public pet pages.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "pets";

#[derive(Default)]
pub struct Pets;

#[feature_async]
impl Feature for Pets {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "Pets",
            description: "Idleon companion collection, pet giveaways and public pet pages.",
            group: Group::Pets,
            nav: vec![NavItem { label: "Pets", path: KEY, icon: Icon::Paw }],
        }
    }
}
