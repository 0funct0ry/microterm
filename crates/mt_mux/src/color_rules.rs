//! `[[tab_bar.color_rules]]` (SPEC §4.4): the first rule that matches any pane of a tab colors it.

use std::collections::HashMap;

use mt_proto::color::TabColor;
use mt_proto::{Resource, RpcError, Selector};

use crate::ctl::{matches, Rows};
use crate::{Id, Mux};

/// Compiled rules, in config order.
#[derive(Debug, Clone, Default)]
pub struct ColorRules {
    rules: Vec<(Selector, TabColor)>,
}

impl ColorRules {
    /// Compiles `(selector, color)` pairs; keys must apply to panes.
    pub fn compile<'a>(
        rules: impl IntoIterator<Item = (&'a str, TabColor)>,
    ) -> Result<ColorRules, RpcError> {
        let mut out = Vec::new();
        for (expr, color) in rules {
            let sel = Selector::parse(expr)?;
            sel.check(Resource::Pane)?;
            out.push((sel, color));
        }
        Ok(ColorRules { rules: out })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// For every tab where some rule matches some pane: `(rule index, color)`. A tab matches a
    /// rule if ANY of its panes does; rules are tried in order and the first match wins.
    pub fn evaluate(&self, rows: &Rows) -> HashMap<Id, (usize, TabColor)> {
        let mut out = HashMap::new();
        if self.rules.is_empty() {
            return out;
        }
        for tab in &rows.tabs {
            let hit = self.rules.iter().enumerate().find(|(_, (sel, _))| {
                rows.panes
                    .iter()
                    .filter(|p| p.tab == tab.id)
                    .any(|p| matches(sel, p))
            });
            if let Some((i, (_, color))) = hit {
                out.insert(tab.id, (i, *color));
            }
        }
        out
    }
}

impl Mux {
    /// Stores the rule colors: tabs in `colors` get theirs, every other tab loses any rule color.
    /// Emits `TabColorChanged` only where the resolved color or source changes.
    pub fn apply_rule_colors(&mut self, colors: &HashMap<Id, (usize, TabColor)>) {
        let tabs: Vec<Id> = self
            .windows()
            .iter()
            .flat_map(|w| w.tabs.iter())
            .map(|t| t.id)
            .collect();
        for tab in tabs {
            let _ = self.set_rule_color(tab, colors.get(&tab).map(|(_, c)| *c));
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use mt_proto::color::NamedColor;

    use super::*;
    use crate::ctl::Live;
    use crate::{ColorSource, MuxEvent};

    const RED: TabColor = TabColor::Named(NamedColor::Red);
    const BLUE: TabColor = TabColor::Named(NamedColor::Blue);

    fn rows(mux: &Mux) -> Rows {
        mux.rows(&|_| Live::default())
    }

    fn rules(list: &[(&str, TabColor)]) -> ColorRules {
        ColorRules::compile(list.iter().copied()).unwrap()
    }

    fn two_tabs() -> (Mux, Id, Id, Id, Id) {
        let mut mux = Mux::new();
        let w = mux.spawn_window(None);
        let (t1, p1) = mux.spawn_tab(w, None, None).unwrap();
        let (t2, p2) = mux.spawn_tab(w, None, None).unwrap();
        (mux, t1, p1, t2, p2)
    }

    #[test]
    fn any_pane_of_the_tab_matches() {
        let (mut mux, t1, p1, t2, _) = two_tabs();
        let extra = mux
            .split_pane_toward(p1, crate::Dir::Right, 0.5, None, None)
            .unwrap();
        mux.set_pane_title(extra, "prod shell");
        let got = rules(&[("title:prod*", RED)]).evaluate(&rows(&mux));
        assert_eq!(got.get(&t1), Some(&(0, RED)));
        assert!(!got.contains_key(&t2));
    }

    #[test]
    fn first_match_wins_in_order() {
        let (mut mux, t1, p1, _, _) = two_tabs();
        mux.set_pane_title(p1, "prod build");
        let r = rules(&[("title:*build*", BLUE), ("title:prod*", RED)]);
        assert_eq!(r.evaluate(&rows(&mux)).get(&t1), Some(&(0, BLUE)));
        let r = rules(&[("title:prod*", RED), ("title:*build*", BLUE)]);
        assert_eq!(r.evaluate(&rows(&mux)).get(&t1), Some(&(0, RED)));
    }

    #[test]
    fn bad_selectors_do_not_compile() {
        assert!(ColorRules::compile([("bogus:x", RED)]).is_err());
        assert!(ColorRules::compile([("no-colon", RED)]).is_err());
    }

    #[test]
    fn rules_follow_title_cwd_and_domain_changes() {
        let (mut mux, t1, p1, _, _) = two_tabs();
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        mux.subscribe(move |e| {
            if let MuxEvent::TabColorChanged { tab, color, source } = e {
                l.lock().unwrap().push((*tab, *color, *source));
            }
        });
        let r = rules(&[("cwd:/srv/*", RED), ("title:deploy", BLUE)]);
        mux.apply_rule_colors(&r.evaluate(&rows(&mux)));
        assert!(log.lock().unwrap().is_empty());

        mux.set_pane_cwd(p1, "/srv/app".into());
        mux.apply_rule_colors(&r.evaluate(&rows(&mux)));
        mux.apply_rule_colors(&r.evaluate(&rows(&mux))); // unchanged: no second event
        assert_eq!(*log.lock().unwrap(), [(t1, Some(RED), ColorSource::Rule)]);

        mux.set_pane_cwd(p1, "/tmp".into());
        mux.set_pane_title(p1, "deploy");
        mux.apply_rule_colors(&r.evaluate(&rows(&mux)));
        mux.set_pane_title(p1, "idle");
        mux.apply_rule_colors(&r.evaluate(&rows(&mux)));
        let seen: Vec<_> = log.lock().unwrap().iter().map(|e| e.1).collect();
        assert_eq!(seen, [Some(RED), Some(BLUE), None]);
    }

    #[test]
    fn domain_rules_see_domain_state_through_the_pane_domain() {
        let mut mux = Mux::new();
        let prod = mux.register_ssh_domain("prod-db");
        let w = mux.spawn_window(None);
        let (local, _) = mux.spawn_tab(w, None, None).unwrap();
        let (remote, _) = mux.spawn_tab_in(w, prod, None, None).unwrap();
        let got = rules(&[("domain:prod-*", RED)]).evaluate(&rows(&mux));
        assert_eq!(got.get(&remote), Some(&(0, RED)));
        assert!(!got.contains_key(&local));
    }

    #[test]
    fn a_manual_color_keeps_priority_over_a_rule() {
        let (mut mux, t1, p1, _, _) = two_tabs();
        mux.set_pane_title(p1, "prod");
        mux.set_tab_color(t1, Some(BLUE)).unwrap();
        mux.apply_rule_colors(&rules(&[("title:prod", RED)]).evaluate(&rows(&mux)));
        assert_eq!(
            mux.tab(t1).unwrap().resolved_color(),
            (Some(BLUE), ColorSource::Manual)
        );
        mux.set_tab_color(t1, None).unwrap();
        assert_eq!(
            mux.tab(t1).unwrap().resolved_color(),
            (Some(RED), ColorSource::Rule)
        );
    }
}
