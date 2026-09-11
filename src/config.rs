//! User rules are resolved against the subject's facts, never a global player.
use serde::Deserialize;
use std::{
    collections::HashSet,
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
};

pub const FILE_NAME: &str = "ERCharacterScale.toml";
pub const DEFAULT_TEXT: &str = include_str!("../ERCharacterScale.toml");
const MAX_FILE_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum TargetKind {
    Player,
    Enemy,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Mode {
    Speffect,
    Constant,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TargetSwitch {
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    version: u32,
    pub enabled: bool,
    pub player: TargetSwitch,
    pub enemies: TargetSwitch,
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub name: String,
    target: TargetKind,
    mode: Mode,
    sp_effect_id: Option<i32>,
    scale: f32,
    character_ids: Option<Vec<u32>>,
    npc_param_ids: Option<Vec<i32>>,
    entity_ids: Option<Vec<u32>>,
    #[serde(default)]
    hostile_only: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct UnitFacts {
    pub kind: TargetKind,
    pub character_id: u32,
    pub npc_param_id: i32,
    pub entity_id: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Selection {
    pub rule: Option<usize>,
    pub scale: f32,
}

impl Default for Selection {
    fn default() -> Self {
        Self {
            rule: None,
            scale: 1.0,
        }
    }
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, String> {
        let parsed: Self = toml::from_str(text.strip_prefix('\u{feff}').unwrap_or(text))
            .map_err(|error| error.to_string())?;
        if parsed.version != 1 {
            return Err(format!(
                "unsupported config version {} (expected 1)",
                parsed.version
            ));
        }
        let mut names = HashSet::new();
        for rule in &parsed.rules {
            let error = |reason| format!("rule {:?}: {reason}", rule.name);
            if rule.name.trim().is_empty() {
                return Err(error("name must not be empty"));
            }
            if !names.insert(&rule.name) {
                return Err(error("duplicate name"));
            }
            if !crate::scale_math::valid(rule.scale) {
                return Err(error("scale must be finite and greater than zero"));
            }
            if rule.hostile_only && rule.target == TargetKind::Player {
                return Err(error("hostile_only=true requires target='enemy'"));
            }
            match (rule.mode, rule.sp_effect_id) {
                (Mode::Speffect, Some(id)) if id >= 0 => (),
                (Mode::Constant, None) => (),
                _ => {
                    return Err(error(
                        "speffect needs a nonnegative sp_effect_id; constant must omit it",
                    ));
                }
            }
            if rule
                .character_ids
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.contains(&u32::MAX))
            {
                return Err(error(
                    "character_ids must be nonempty and contain valid model numbers",
                ));
            }
            if rule
                .npc_param_ids
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.iter().any(|id| *id < 0))
            {
                return Err(error("npc_param_ids must be nonempty and nonnegative"));
            }
            if rule
                .entity_ids
                .as_ref()
                .is_some_and(|v| v.is_empty() || v.iter().any(|id| *id == 0 || *id == u32::MAX))
            {
                return Err(error(
                    "entity_ids must be nonempty and exclude 0 and 4294967295",
                ));
            }
        }
        Ok(parsed)
    }

    #[cfg(test)]
    pub fn resolve(&self, facts: UnitFacts, has_effect: impl FnMut(i32) -> bool) -> Selection {
        self.resolve_with_relation(facts, has_effect, || None)
    }

    pub fn resolve_with_relation(
        &self,
        facts: UnitFacts,
        mut has_effect: impl FnMut(i32) -> bool,
        mut is_hostile: impl FnMut() -> Option<bool>,
    ) -> Selection {
        if !self.enabled
            || !match facts.kind {
                TargetKind::Player => self.player.enabled,
                TargetKind::Enemy => self.enemies.enabled,
            }
        {
            return Selection::default();
        }
        let mut relation = None;
        for (index, rule) in self.rules.iter().enumerate() {
            if rule.target != facts.kind
                || !matches(&rule.character_ids, facts.character_id)
                || !matches(&rule.npc_param_ids, facts.npc_param_id)
                || !matches(&rule.entity_ids, facts.entity_id)
            {
                continue;
            }
            if rule.mode == Mode::Constant || rule.sp_effect_id.is_some_and(&mut has_effect) {
                // Ordinary model rules never depend on native team lookup.
                // Unknown relation only skips explicitly hostile-only rules.
                if rule.hostile_only && *relation.get_or_insert_with(&mut is_hostile) != Some(true)
                {
                    continue;
                }
                return Selection {
                    rule: Some(index),
                    scale: rule.scale,
                };
            }
        }
        Selection::default()
    }
}

fn matches<T: PartialEq>(filter: &Option<Vec<T>>, value: T) -> bool {
    filter.as_ref().is_none_or(|values| values.contains(&value))
}

pub struct Loaded {
    pub config: Config,
    pub message: String,
}

fn read_existing(path: &Path) -> Result<Loaded, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut text = String::new();
    file.take(MAX_FILE_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if text.len() as u64 > MAX_FILE_BYTES {
        return Err("config exceeds 1 MiB".into());
    }
    Ok(Loaded {
        config: Config::parse(&text)?,
        message: format!("loaded {}", path.display()),
    })
}

pub fn load(path: &Path) -> Result<Loaded, String> {
    match std::fs::metadata(path) {
        Ok(_) => return read_existing(path),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(format!("{}: {error}", path.display())),
    }
    let config = Config::parse(DEFAULT_TEXT)?;
    let message = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(mut file) => match file
            .write_all(DEFAULT_TEXT.as_bytes())
            .and_then(|_| file.flush())
        {
            Ok(()) => format!("created compatibility defaults at {}", path.display()),
            Err(error) => {
                // We own this newly created file. A failed write must not leave
                // a truncated template that disables the next launch.
                drop(file);
                let _ = std::fs::remove_file(path);
                format!(
                    "using built-in compatibility defaults; could not write {}: {error}",
                    path.display()
                )
            }
        },
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_existing(path);
        }
        Err(error) => format!(
            "using built-in compatibility defaults; could not create {}: {error}",
            path.display()
        ),
    };
    Ok(Loaded { config, message })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_hostility_is_a_rule_filter_with_ordered_fallback() {
        let c = Config::parse(&text(
            &(rule(
                "hostile",
                "enemy",
                "mode='constant'\nscale=0.6\ncharacter_ids=[1234]\nhostile_only=true",
            ) + &rule(
                "other",
                "enemy",
                "mode='constant'\nscale=0.8\ncharacter_ids=[1234]",
            )),
        ))
        .unwrap();
        for (relation, scale, index) in
            [(Some(true), 0.6, 0), (Some(false), 0.8, 1), (None, 0.8, 1)]
        {
            assert_eq!(
                c.resolve_with_relation(
                    facts(TargetKind::Enemy, 1),
                    |_| panic!("constant"),
                    || relation
                ),
                Selection {
                    rule: Some(index),
                    scale
                }
            );
        }
        assert!(
            Config::parse(&text(&rule(
                "invalid",
                "player",
                "mode='constant'\nscale=0.6\nhostile_only=true"
            )))
            .is_err()
        );
    }

    #[test]
    fn model_only_rules_do_not_query_or_require_hostility() {
        for extra in ["", "\nhostile_only=false"] {
            let c = Config::parse(&text(&rule(
                "model",
                "enemy",
                &format!("mode='constant'\nscale=0.6\ncharacter_ids=[1234]{extra}"),
            )))
            .unwrap();
            assert_eq!(
                c.resolve_with_relation(
                    facts(TargetKind::Enemy, 1),
                    |_| panic!("constant"),
                    || panic!("model rule queried hostility")
                )
                .scale,
                0.6
            );
            let mut different = facts(TargetKind::Enemy, 2);
            different.character_id = 9521;
            assert_eq!(
                c.resolve_with_relation(
                    different,
                    |_| panic!("different model"),
                    || panic!("different model")
                )
                .scale,
                1.0
            );
        }
    }

    #[test]
    fn shipped_examples_cover_non_c0000_without_requiring_effects() {
        let enemy = UnitFacts {
            kind: TargetKind::Enemy,
            character_id: 3250,
            npc_param_id: 1234,
            entity_id: 100,
        };
        for text in [
            include_str!("../examples/ERCharacterScale.enemies-half.toml"),
            include_str!("../examples/ERCharacterScale.fixed.toml"),
        ] {
            let config = Config::parse(text).unwrap();
            assert_eq!(
                config
                    .resolve(enemy, |_| panic!("fixed enemy rule must not query effects"))
                    .scale,
                0.5
            );
        }
        let fixed = Config::parse(include_str!("../examples/ERCharacterScale.fixed.toml")).unwrap();
        assert_eq!(
            fixed
                .resolve(
                    UnitFacts {
                        kind: TargetKind::Player,
                        ..enemy
                    },
                    |_| panic!("fixed player")
                )
                .scale,
            0.75
        );
    }

    fn facts(kind: TargetKind, entity_id: u32) -> UnitFacts {
        UnitFacts {
            kind,
            character_id: 1234,
            npc_param_id: 123456,
            entity_id,
        }
    }
    fn text(rules: &str) -> String {
        format!(
            "version = 1\nenabled = true\n[player]\nenabled = true\n[enemies]\nenabled = true\n{rules}"
        )
    }
    fn rule(name: &str, target: &str, more: &str) -> String {
        format!("[[rules]]\nname = {name:?}\ntarget = {target:?}\n{more}\n")
    }

    #[test]
    fn shipped_defaults_preserve_all_253_mappings_and_order() {
        let c = Config::parse(DEFAULT_TEXT).unwrap();
        let expected = [
            (21202000, 1.05),
            (21202001, 1.1),
            (21202002, 1.15),
            (21202003, 1.2),
            (21202004, 1.25),
            (21202005, 1.3),
            (21202050, 0.95),
            (21202051, 0.9),
            (21202052, 0.85),
            (21202053, 0.8),
            (21202054, 0.75),
            (21202055, 0.7),
            (8020400, 0.5),
            (8020401, 3.0),
        ];
        assert_eq!(c.rules.len(), expected.len());
        assert!(!c.enemies.enabled);
        for (index, (effect, scale)) in expected.into_iter().enumerate() {
            assert_eq!(
                c.resolve(facts(TargetKind::Player, 0), |id| id == effect),
                Selection {
                    rule: Some(index),
                    scale
                }
            );
        }
        assert_eq!(
            c.resolve(facts(TargetKind::Player, 0), |_| true).scale,
            1.05
        );
        assert_eq!(
            c.resolve(facts(TargetKind::Player, 0), |_| false),
            Selection::default()
        );
    }

    #[test]
    fn fixed_rule_never_queries_effects() {
        let c = Config::parse(&text(&rule(
            "fixed",
            "player",
            "mode = 'constant'\nscale = 0.75",
        )))
        .unwrap();
        assert_eq!(
            c.resolve(facts(TargetKind::Player, 0), |_| panic!(
                "effect query in constant mode"
            ))
            .scale,
            0.75
        );
    }

    #[test]
    fn same_effect_is_resolved_for_each_subject_and_falls_through_in_file_order() {
        let rules = rule(
            "p",
            "player",
            "mode='speffect'\nsp_effect_id=8020400\nscale=0.5",
        ) + &rule(
            "a1",
            "enemy",
            "mode='speffect'\nsp_effect_id=8020400\nscale=1.5\nentity_ids=[101]",
        ) + &rule(
            "a2",
            "enemy",
            "mode='constant'\nscale=0.75\nentity_ids=[102]",
        ) + &rule(
            "a1-fallback",
            "enemy",
            "mode='constant'\nscale=1.25\nentity_ids=[101]",
        );
        let c = Config::parse(&text(&rules)).unwrap();
        for (kind, id, active, scale) in [
            (TargetKind::Player, 0, true, 0.5),
            (TargetKind::Enemy, 101, true, 1.5),
            (TargetKind::Enemy, 102, false, 0.75),
            (TargetKind::Enemy, 103, true, 1.0),
            (TargetKind::Enemy, 101, false, 1.25),
        ] {
            assert_eq!(c.resolve(facts(kind, id), |_| active).scale, scale);
        }
    }

    #[test]
    fn filters_require_all_fields_and_any_member_within_a_list() {
        let c = Config::parse(&text(&rule("filter", "enemy", "mode='constant'\nscale=2.0\ncharacter_ids=[1234,2345]\nnpc_param_ids=[123456]\nentity_ids=[101,102]"))).unwrap();
        let good = facts(TargetKind::Enemy, 102);
        assert_eq!(c.resolve(good, |_| false).scale, 2.0);
        for bad in [
            UnitFacts {
                kind: TargetKind::Player,
                ..good
            },
            UnitFacts {
                character_id: 7777,
                ..good
            },
            UnitFacts {
                npc_param_id: 9,
                ..good
            },
            UnitFacts {
                entity_id: 103,
                ..good
            },
        ] {
            assert_eq!(
                c.resolve(bad, |_| panic!("nonmatching unit queried")).scale,
                1.0
            );
        }
    }

    #[test]
    fn invalid_values_and_unknown_fields_are_rejected() {
        for value in ["0", "-0.0", "-1", "nan", "inf", "-inf", "1e100", "1e-100"] {
            assert!(
                Config::parse(&text(&rule(
                    "bad",
                    "player",
                    &format!("mode='constant'\nscale={value}")
                )))
                .is_err(),
                "{value}"
            );
        }
        for more in [
            "mode='constant'\nscale=1.0\nsp_effect_id=1",
            "mode='speffect'\nscale=1.0",
            "mode='speffect'\nscale=1.0\nsp_effect_id=-1",
            "mode='constant'\nscale=1.0\nentity_ids=[]",
            "mode='constant'\nscale=1.0\nentity_ids=[0]",
            "mode='constant'\nscale=1.0\nentity_ids=[4294967295]",
            "mode='constant'\nscale=1.0\nnpc_param_ids=[-1]",
            "mode='constant'\nscale=1.0\ncharacter_ids=[]",
            "mode='constant'\nscale=1.0\nscalle=2.0",
        ] {
            assert!(
                Config::parse(&text(&rule("bad", "enemy", more))).is_err(),
                "{more}"
            );
        }
        let r = rule("duplicate", "player", "mode='constant'\nscale=1.0");
        assert!(Config::parse(&text(&(r.clone() + &r))).is_err());
        assert!(Config::parse(&text(&r).replacen("version = 1", "version = 2", 1)).is_err());
        assert!(Config::parse("broken = [").is_err());
    }

    #[test]
    fn bom_and_positive_scales_outside_old_range_are_valid() {
        assert!(Config::parse(&format!("\u{feff}{DEFAULT_TEXT}")).is_ok());
        for scale in [
            0.1,
            0.25,
            0.49,
            0.5,
            3.0,
            3.01,
            4.0,
            10.0,
            f32::MIN_POSITIVE,
            f32::MAX,
        ] {
            assert!(
                Config::parse(&text(&rule(
                    "endpoint",
                    "enemy",
                    &format!("mode='constant'\nscale={scale:e}")
                )))
                .is_ok()
            );
        }
    }

    #[test]
    fn disabled_categories_do_not_query_effects() {
        let mut c = Config::parse(DEFAULT_TEXT).unwrap();
        assert_eq!(
            c.resolve(facts(TargetKind::Enemy, 1), |_| panic!()),
            Selection::default()
        );
        c.player.enabled = false;
        assert_eq!(
            c.resolve(facts(TargetKind::Player, 0), |_| panic!()),
            Selection::default()
        );
        c.player.enabled = true;
        c.enabled = false;
        assert_eq!(
            c.resolve(facts(TargetKind::Player, 0), |_| panic!()),
            Selection::default()
        );
    }

    #[test]
    fn missing_file_creates_defaults_existing_file_is_never_overwritten_and_invalid_disables() {
        let directory = std::env::temp_dir().join(format!(
            "er-character-scale-config-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&directory).unwrap();
        let path = directory.join(FILE_NAME);
        assert!(load(&path).unwrap().message.contains("created"));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), DEFAULT_TEXT);
        let custom = text(&rule("custom", "player", "mode='constant'\nscale=0.8"));
        std::fs::write(&path, &custom).unwrap();
        assert_eq!(
            load(&path)
                .unwrap()
                .config
                .resolve(facts(TargetKind::Player, 0), |_| false)
                .scale,
            0.8
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), custom);
        std::fs::write(&path, "bad syntax !").unwrap();
        assert!(load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "bad syntax !");
        // An absent parent makes default creation impossible, like a protected
        // installation directory, without depending on the test user's ACLs.
        assert!(
            load(&directory.join("absent").join(FILE_NAME))
                .unwrap()
                .message
                .contains("could not create")
        );
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(directory).unwrap();
    }
}
