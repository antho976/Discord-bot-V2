//! Lets Relay, Tally, Avex and your PCs connect after an approver allows them in Discord.

use bot_core::{Feature, FeatureMeta, Group, Icon, NavItem, feature_async};

pub const KEY: &str = "connect";

#[derive(Default)]
pub struct Connect;

#[feature_async]
impl Feature for Connect {
    fn meta(&self) -> FeatureMeta {
        FeatureMeta {
            key: KEY,
            name: "App approvals",
            description: "Lets Relay, Tally, Avex and your PCs connect after an approver allows them in Discord.",
            group: Group::Tools,
            nav: vec![NavItem { label: "App approvals", path: KEY, icon: Icon::Plug }],
        }
    }
}
