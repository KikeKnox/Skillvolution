//! Client versions: what changes between releases of a client (`Caps`, per
//! `VersionRange`), and how the installed version is found. Only `setup` and `doctor`
//! detect it, by running `<cli> --version`; everything else reads the version setup
//! stored in the vault (`client_versions`), because hooks must never pay for a process
//! spawn on every event.

use super::{ClientKind, DelegationTool, ModelRefFormat};
use crate::setup::find_on_path;
use std::{
    fmt,
    io::Read,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

/// How long `<cli> --version` may run before it is killed and the client counts as not
/// detected.
const DETECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A client version: up to three numeric components (a missing one is 0), so `2.2.5`,
/// `3000.11.3` and a date version like `2025.11.06-abc123` all order correctly.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// The first dotted number in `text` (`opencode v2.2.5`, `2.1.332 (Claude Code)`,
    /// `codex-cli 0.46.0`); `None` when there is none.
    pub fn parse(text: &str) -> Option<Version> {
        let bytes = text.as_bytes();
        let mut start = 0;
        while start < bytes.len() {
            if !bytes[start].is_ascii_digit() {
                start += 1;
                continue;
            }
            let end = bytes[start..]
                .iter()
                .position(|b| !(b.is_ascii_digit() || *b == b'.'))
                .map_or(bytes.len(), |n| start + n);
            let token = text[start..end].trim_end_matches('.');
            let parts: Vec<&str> = token.split('.').collect();
            if (2..=4).contains(&parts.len())
                && let Some(numbers) = parts
                    .iter()
                    .take(3)
                    .map(|part| part.parse::<u32>().ok())
                    .collect::<Option<Vec<u32>>>()
            {
                let at = |i: usize| numbers.get(i).copied().unwrap_or(0);
                return Some(Version(at(0), at(1), at(2)));
            }
            start = end;
        }
        None
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// How a client's plugin is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PluginFormat {
    /// The client has no plugin; its hooks are commands.
    None,
    /// `export default { id, server }` (or named function exports).
    ServerExport,
    /// `export default { id, setup }`.
    SetupExport,
}

/// What differs between versions of a client.
#[derive(Clone, Copy, Debug)]
pub struct Caps {
    /// The tools that delegate to a subagent, with the input field naming the agent.
    pub delegation: &'static [DelegationTool],
    pub plugin: PluginFormat,
    /// How a delegation call can name a model.
    pub per_call_model: ModelRefFormat,
    /// Whether the client picks up agent file changes without a restart.
    pub hot_reload: bool,
}

impl Caps {
    /// A client with no delegation tools, plugin, per-call model or hot reload known.
    pub const NONE: Caps = Caps {
        delegation: &[],
        plugin: PluginFormat::None,
        per_call_model: ModelRefFormat::None,
        hot_reload: false,
    };
}

/// The capabilities from `min` (inclusive) up to `max` (exclusive; `None` is open).
pub struct VersionRange {
    pub min: Version,
    pub max: Option<Version>,
    pub caps: Caps,
}

impl VersionRange {
    fn contains(&self, version: Version) -> bool {
        version >= self.min && self.max.is_none_or(|max| version < max)
    }
}

/// A client's known versions. `ranges` are contiguous, oldest first, the last open.
pub struct VersionSpec {
    pub ranges: &'static [VersionRange],
    /// The newest version Skillvolution was tried against; a newer one is "unverified".
    /// `None` when none is recorded, which also means nothing is ever unverified.
    pub last_tested: Option<Version>,
}

/// The version the first range starts at.
pub const ANY: Version = Version(0, 0, 0);

impl ClientKind {
    /// The capabilities for `version`; `None` (no CLI, or its output not recognised)
    /// assumes the latest known version.
    pub fn caps(self, version: Option<Version>) -> &'static Caps {
        let ranges = self.spec().versions.ranges;
        let latest = &ranges[ranges.len() - 1];
        let range = match version {
            Some(version) => ranges
                .iter()
                .find(|range| range.contains(version))
                .unwrap_or(latest),
            None => latest,
        };
        &range.caps
    }

    /// Whether `version` is newer than the last one Skillvolution was tested against.
    pub fn is_unverified(self, version: Version) -> bool {
        self.spec()
            .versions
            .last_tested
            .is_some_and(|tested| version > tested)
    }

    /// The installed version, from `<cli> --version` (see `detect_version_with`).
    pub fn detect_version(self) -> Option<Version> {
        self.detect_version_with(run_version)
    }

    /// The first parsable version `run` reports for one of the client's CLIs. `run` gets
    /// the CLI's name and returns its `--version` output, or `None` when it is not
    /// installed, fails, or takes too long.
    pub fn detect_version_with(self, run: impl Fn(&str) -> Option<String>) -> Option<Version> {
        self.spec()
            .clis
            .iter()
            .find_map(|cli| run(cli).and_then(|output| Version::parse(&output)))
    }
}

/// Runs `<cli> --version` for a CLI found on `PATH`, killing it after `DETECT_TIMEOUT`.
/// Standard output and error both count, since clients differ on where they print it.
fn run_version(cli: &str) -> Option<String> {
    let path = find_on_path(cli)?;
    let mut child = Command::new(path)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + DETECT_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    let mut output = String::new();
    if let Some(stdout) = child.stdout.take() {
        let _ = stdout.take(64 * 1024).read_to_string(&mut output);
    }
    if let Some(stderr) = child.stderr.take() {
        let _ = stderr.take(64 * 1024).read_to_string(&mut output);
    }
    Some(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_version_each_installed_client_prints() {
        for (output, expected) in [
            ("2.1.332 (Claude Code)\n", Version(2, 1, 332)),
            ("opencode v2.2.5\n", Version(2, 2, 5)),
            ("devin 3000.11.3 (9c803229faa4)\n", Version(3000, 11, 3)),
            ("codex-cli 0.46.0", Version(0, 46, 0)),
            ("2025.11.06-8fe8a63", Version(2025, 11, 6)),
            ("1.2", Version(1, 2, 0)),
            ("build 7 of 1.19.6", Version(1, 19, 6)),
        ] {
            assert_eq!(Version::parse(output), Some(expected), "{output:?}");
        }
        for output in ["", "unknown", "version 12", "usage: x --help", "1."] {
            assert_eq!(Version::parse(output), None, "{output:?}");
        }
    }

    #[test]
    fn versions_order_numerically_not_as_text() {
        assert!(Version(2, 1, 332) > Version(2, 1, 33));
        assert!(Version(1, 19, 6) < Version(2, 0, 0));
        assert_eq!(Version(2, 2, 5).to_string(), "2.2.5");
    }

    #[test]
    fn every_clients_ranges_are_contiguous_and_cover_every_version() {
        for kind in ClientKind::ALL {
            let spec = kind.spec();
            let ranges = spec.versions.ranges;
            assert!(!ranges.is_empty(), "{}: no version range", spec.token);
            assert_eq!(ranges[0].min, ANY, "{}: first range", spec.token);
            for pair in ranges.windows(2) {
                assert_eq!(
                    pair[0].max,
                    Some(pair[1].min),
                    "{}: a gap or overlap",
                    spec.token
                );
            }
            assert_eq!(
                ranges[ranges.len() - 1].max,
                None,
                "{}: last range",
                spec.token
            );
            let probes = [
                Version(0, 0, 1),
                Version(1, 19, 6),
                Version(2, 0, 0),
                Version(9999, 0, 0),
            ];
            for probe in probes {
                assert_eq!(
                    ranges.iter().filter(|range| range.contains(probe)).count(),
                    1,
                    "{}: {probe}",
                    spec.token
                );
            }
        }
    }

    #[test]
    fn hook_delegation_tools_are_the_union_of_every_ranges() {
        for kind in ClientKind::ALL {
            let spec = kind.spec();
            let mut from_caps: Vec<(&str, &str)> = spec
                .versions
                .ranges
                .iter()
                .flat_map(|range| range.caps.delegation)
                .map(|d| (d.tool, d.agent_field))
                .collect();
            let mut from_hooks: Vec<(&str, &str)> = spec
                .hooks
                .delegation_tools
                .iter()
                .map(|d| (d.tool, d.agent_field))
                .collect();
            from_caps.sort_unstable();
            from_caps.dedup();
            from_hooks.sort_unstable();
            assert_eq!(from_caps, from_hooks, "{}", spec.token);
        }
    }

    #[test]
    fn opencode_caps_follow_the_version() {
        let kind = ClientKind::OpenCode;
        let v1 = kind.caps(Some(Version(1, 19, 6)));
        assert_eq!(v1.delegation[0].tool, "task");
        assert_eq!(v1.delegation[0].agent_field, "subagent_type");
        assert_eq!(v1.plugin, PluginFormat::ServerExport);
        assert_eq!(v1.per_call_model, ModelRefFormat::None);
        assert!(!v1.hot_reload);

        let v2 = kind.caps(Some(Version(2, 2, 5)));
        assert_eq!(v2.delegation[0].tool, "subagent");
        assert_eq!(v2.delegation[0].agent_field, "agent");
        assert_eq!(v2.plugin, PluginFormat::SetupExport);
        assert_eq!(v2.per_call_model, ModelRefFormat::ProviderModel);
        assert!(v2.hot_reload);

        // No version known: the latest one's capabilities.
        assert_eq!(kind.caps(None).plugin, PluginFormat::SetupExport);
        // A version far beyond the last tested one still gets the newest range.
        assert!(kind.caps(Some(Version(9, 0, 0))).hot_reload);
        assert!(kind.is_unverified(Version(2, 2, 6)));
        assert!(!kind.is_unverified(Version(2, 2, 5)));
    }

    #[test]
    fn claude_code_keeps_per_call_alias_models_and_hot_reload() {
        let caps = ClientKind::ClaudeCode.caps(None);
        assert_eq!(caps.per_call_model, ModelRefFormat::Alias);
        assert!(caps.hot_reload);
    }

    #[test]
    fn detection_takes_the_first_cli_with_a_parsable_version() {
        let kind = ClientKind::Cursor;
        let clis = kind.spec().clis;
        assert!(clis.len() > 1);
        assert_eq!(kind.detect_version_with(|_| None), None);
        assert_eq!(kind.detect_version_with(|_| Some("garbage".into())), None);
        let second = clis[1];
        let found = kind.detect_version_with(|cli| {
            Some(if cli == second { "9.8.7" } else { "no version" }.to_owned())
        });
        assert_eq!(found, Some(Version(9, 8, 7)));
    }

    #[test]
    fn clients_without_a_recorded_tested_version_are_never_unverified() {
        let spec = ClientKind::Codex.spec();
        assert_eq!(spec.versions.last_tested, None);
        assert!(!ClientKind::Codex.is_unverified(Version(999, 0, 0)));
    }
}
