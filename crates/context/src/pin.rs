//! JS: pin.mjs -> `impeccable pin <pin|unpin> <command>`

use crate::jsp;
use crate::provider::{self, Provider};
use crate::util::{exists, read_json, safe_read};
use impeccable_common::Io;

/// Bundled copy of `skill/scripts/command-metadata.json` (build-time copy;
/// keep in sync with the public repo).
pub const COMMAND_METADATA_JSON: &str = include_str!("command-metadata.json");

const HARNESS_DIRS: [&str; 20] = [
    ".claude", ".cursor", ".devin", ".dsh", ".gemini", ".codex", ".agents", ".agent", ".github", ".grok", ".hermes", ".trae", ".trae-cn",
    ".pi", ".omp", ".opencode", ".kiro", ".rovodev", ".vibe", ".qoder",
];
const CODEX_HARNESSES: [&str; 2] = [".codex", ".agents"];
pub const VALID_COMMANDS: [&str; 24] = [
    "craft", "init", "extract", "document", "shape", "critique", "audit", "polish", "bolder", "quieter", "distill",
    "harden", "onboard", "live", "animate", "colorize", "typeset", "layout", "delight", "overdrive", "clarify",
    "adapt", "optimize", "generate",
];
const PIN_MARKER: &str = "<!-- impeccable-pinned-skill -->";
const OPENCODE_PIN_MARKER: &str = "<!-- impeccable-pinned-command -->";

fn find_project_root(start: &str) -> String {
    let mut dir = jsp::resolve(start, &[]);
    while dir != "/" {
        if exists(&jsp::join(&[&dir, "package.json"]))
            || exists(&jsp::join(&[&dir, ".git"]))
            || exists(&jsp::join(&[&dir, "skills-lock.json"]))
        {
            return dir;
        }
        let parent = jsp::resolve(&dir, &[".."]);
        if parent == dir {
            break;
        }
        dir = parent;
    }
    jsp::resolve(start, &[])
}

fn find_harness_dirs(project_root: &str) -> Vec<String> {
    let mut dirs = Vec::new();
    for h in HARNESS_DIRS {
        let skills = jsp::join(&[project_root, h, "skills"]);
        if exists(&jsp::join(&[&skills, "impeccable"])) || exists(&jsp::join(&[&skills, "i-impeccable"])) {
            dirs.push(skills);
        }
    }
    dirs
}

fn command_prefix_for(skills_dir: &str) -> &'static str {
    let harness = jsp::basename(&jsp::dirname(skills_dir));
    if CODEX_HARNESSES.contains(&harness.as_str()) {
        "$"
    } else {
        "/"
    }
}

fn generate_pinned_skill(command: &str, metadata: &serde_json::Value, prefix: &str, is_codex: bool) -> String {
    let entry = metadata.get(command);
    let desc = entry
        .and_then(|e| e.get("description"))
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Shortcut for {}impeccable {}.", prefix, command));
    let hint = entry
        .and_then(|e| e.get("argumentHint"))
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("[target]");
    let provider_fm = if is_codex {
        format!("metadata:\n  argument-hint: \"{}\"", hint)
    } else {
        format!("argument-hint: \"{}\"\nuser-invocable: true", hint)
    };
    format!(
        "---\nname: {command}\ndescription: \"{desc}\"\n{provider_fm}\n---\n\n{marker}\n\nThis is a pinned shortcut for `{prefix}impeccable {command}`.\n\nInvoke {prefix}impeccable {command}, passing along any arguments provided here, and follow its instructions.\n",
        command = command,
        desc = desc,
        provider_fm = provider_fm,
        marker = PIN_MARKER,
        prefix = prefix
    )
}

// OpenCode 1.18.10 does not honor `user-invocable: true` on SKILL.md
// frontmatter (see docs/HARNESSES.md), so a pinned skill there shows up in
// `opencode debug skill` but never in the slash menu. The fix is a sibling
// `commands/impeccable-<cmd>.md` on the OpenCode command schema. The body
// loads the skill, runs the context verb, then reads the sub-command's
// reference file, so `/impeccable-<cmd>` runs the same workflow
// `/impeccable <cmd>` routes to.
//
// JS: pin.mjs#generatePinnedOpencodeCommand. The JS body says
// `node <skill-base-dir>/scripts/context.mjs`; the engine names its own
// command, as everywhere else the launcher replaced a script path.
fn generate_pinned_opencode_command(command: &str, metadata: &serde_json::Value) -> String {
    let desc = metadata
        .get(command)
        .and_then(|e| e.get("description"))
        .and_then(|d| d.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            format!("Impeccable sub-command shortcut; runs the {command} workflow via /impeccable.")
        });
    format!(
        "---\ndescription: \"{desc}\"\nagent: build\nsubtask: true\n---\n\n{marker}\n\nLoad the `impeccable` skill via the skill tool (name: \"impeccable\"), then run `<skill-base-dir>/scripts/impeccable context`, then load `<skill-base-dir>/reference/{command}.md` and follow it. `<skill-base-dir>` is the skill's base directory as reported by the skill tool response; substitute the actual absolute path before running or reading anything.\n\n$ARGUMENTS\n",
        desc = desc,
        marker = OPENCODE_PIN_MARKER,
        command = command
    )
}

/// JS: pin.mjs#opencodeUserConfigDir. Mirrors the CLI's precedence
/// (`OPENCODE_CONFIG_DIR` -> `XDG_CONFIG_HOME/opencode` -> `~/.config/opencode`).
fn opencode_user_config_dir(io: &Io) -> String {
    opencode_user_config_dir_from(&io.env)
}

/// The same resolution for callers that hold an `Env` rather than an `Io`,
/// so the CLI's install destination, `pin` and `context` cannot disagree.
pub(crate) fn opencode_user_config_dir_from(env: &crate::util::Env) -> String {
    if let Some(v) = env.get("OPENCODE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        return v.clone();
    }
    if let Some(v) = env.get("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return jsp::join(&[v, "opencode"]);
    }
    jsp::join(&[&crate::util::homedir(env), ".config", "opencode"])
}

/// JS: pin.mjs#findOpencodeCommandsDirs. The project-local dir when the
/// project has the skill, plus the user config dir when Impeccable is
/// installed globally. With `for_cleanup`, both are included even when the
/// skill is gone, so unpin can still reach a pin a removed install left
/// behind; removal stays safe because it is marker-guarded.
fn find_opencode_commands_dirs(project_root: &str, io: &Io, for_cleanup: bool) -> Vec<String> {
    let mut dirs: Vec<String> = Vec::new();
    let mut push = |dir: String| {
        let key = jsp::resolve(&dir, &[]);
        if !dirs.iter().any(|d| jsp::resolve(d, &[]) == key) {
            dirs.push(dir);
        }
    };
    if for_cleanup || exists(&jsp::join(&[project_root, ".opencode", "skills", "impeccable"])) {
        push(jsp::join(&[project_root, ".opencode", "commands"]));
    }
    let user_config = opencode_user_config_dir(io);
    if for_cleanup || exists(&jsp::join(&[&user_config, "skills", "impeccable"])) {
        push(jsp::join(&[&user_config, "commands"]));
    }
    dirs
}

/// JS: pin.mjs#writePinnedOpencodeCommand
fn write_pinned_opencode_command(
    commands_dir: &str,
    command: &str,
    metadata: &serde_json::Value,
    io: &mut Io,
) -> bool {
    let command_file = jsp::join(&[commands_dir, &format!("impeccable-{command}.md")]);
    if exists(&command_file) {
        let existing = safe_read(&command_file).unwrap_or_default();
        if !existing.contains(OPENCODE_PIN_MARKER) {
            io.out(&format!(
                "  SKIP: {} (non-pinned command already exists)\n",
                command_file
            ));
            return false;
        }
    } else {
        let _ = std::fs::create_dir_all(commands_dir);
    }
    let _ = std::fs::write(&command_file, generate_pinned_opencode_command(command, metadata));
    io.out(&format!("  + {}\n", command_file));
    true
}

/// JS: pin.mjs#removePinnedOpencodeCommand
fn remove_pinned_opencode_command(commands_dir: &str, command: &str, io: &mut Io) -> bool {
    let command_file = jsp::join(&[commands_dir, &format!("impeccable-{command}.md")]);
    if !exists(&command_file) {
        return false;
    }
    let content = safe_read(&command_file).unwrap_or_default();
    if !content.contains(OPENCODE_PIN_MARKER) {
        io.out(&format!("  SKIP: {} (not a pinned command)\n", command_file));
        return false;
    }
    let _ = std::fs::remove_file(&command_file);
    io.out(&format!("  - {}\n", command_file));
    true
}

/// JS `skillsDir.includes(`${sep}.opencode${sep}`)`.
fn is_opencode_skills_dir(skills_dir: &str) -> bool {
    skills_dir.contains(&format!("{sep}.opencode{sep}", sep = jsp::SEP))
}

fn load_metadata(io: &Io) -> serde_json::Value {
    // Prefer a sibling command-metadata.json in the skill dir when present
    // (an installed skill may be newer than the embedded copy).
    let cwd = io.cwd.to_string_lossy().into_owned();
    let provider = crate::provider::detect(&io.env, &cwd);
    if let Some(dir) = &provider.skill_dir {
        let p = jsp::join(&[dir, "scripts", "command-metadata.json"]);
        if let Some(v) = read_json(&p) {
            return v;
        }
    }
    serde_json::from_str(COMMAND_METADATA_JSON).unwrap_or(serde_json::Value::Object(Default::default()))
}

/// Where one pinned shortcut lands.
#[derive(Clone)]
enum Target {
    /// `<skills_dir>/<command>/SKILL.md`, the surface every skill-reading
    /// harness turns into a slash command (OpenCode is the exception).
    Skill { skills_dir: String },
    /// `<commands_dir>/impeccable-<command>.md`, OpenCode's slash surface.
    Opencode { commands_dir: String },
}

impl Target {
    fn key(&self) -> &str {
        match self {
            Target::Skill { skills_dir } => skills_dir,
            Target::Opencode { commands_dir } => commands_dir,
        }
    }
}

/// The install this run is using, when an agent loaded one.
///
/// Which skill an agent exposes is the agent's call; the copy it runs is named
/// by `IMPECCABLE_SKILL_DIR`, which the launcher exports from its own path. A
/// source checkout has no harness folder (`provider.id == "source"`), so the
/// caller falls back to scanning the project there.
fn running_target(provider: &Provider, io: &Io) -> Option<Target> {
    if provider.id == provider::SOURCE_PROVIDER {
        return None;
    }
    Some(target_for_skill_dir(provider.skill_dir.as_deref()?, io))
}

/// `<root>/.opencode/skills/impeccable` -> `<root>/.opencode/commands`, and a
/// user install at `<config>/skills/impeccable` -> `<config>/commands`.
/// Anything else keeps its skill shortcut: a shared `.agents` copy belongs to
/// the skill-reading harnesses, and OpenCode's slash surface belongs to an
/// OpenCode install.
fn target_for_skill_dir(skill_dir: &str, io: &Io) -> Target {
    let skills_dir = jsp::dirname(skill_dir);
    let harness_root = jsp::dirname(&skills_dir);
    let user_config = opencode_user_config_dir(io);
    let opencode = jsp::basename(&harness_root) == ".opencode"
        || jsp::resolve(&harness_root, &[]) == jsp::resolve(&user_config, &[]);
    if opencode {
        Target::Opencode { commands_dir: jsp::join(&[&harness_root, "commands"]) }
    } else {
        Target::Skill { skills_dir }
    }
}

/// Every install the project scan can find, used when no agent copy answers for
/// this run (a source checkout, or `pin` from a plain shell).
fn scanned_targets(root: &str, io: &Io, for_cleanup: bool) -> Vec<Target> {
    let mut targets: Vec<Target> = Vec::new();
    let push = |target: Target, targets: &mut Vec<Target>| {
        if !targets.iter().any(|t| t.key() == target.key()) {
            targets.push(target);
        }
    };
    for skills_dir in find_harness_dirs(root) {
        // OpenCode's shortcut is the command written below; a
        // `.opencode/skills/<cmd>/SKILL.md` would be one it never surfaces.
        if is_opencode_skills_dir(&skills_dir) {
            continue;
        }
        push(Target::Skill { skills_dir }, &mut targets);
    }
    for commands_dir in find_opencode_commands_dirs(root, io, for_cleanup) {
        push(Target::Opencode { commands_dir }, &mut targets);
    }
    targets
}

/// JS: pin.mjs#writePinnedSkill
fn write_pinned_skill(skills_dir: &str, command: &str, metadata: &serde_json::Value, io: &mut Io) -> bool {
    let prefix = command_prefix_for(skills_dir);
    let content = generate_pinned_skill(command, metadata, prefix, prefix == "$");
    let skill_dir = jsp::join(&[skills_dir, command]);
    if exists(&skill_dir) {
        let md = jsp::join(&[&skill_dir, "SKILL.md"]);
        if exists(&md) {
            let existing = safe_read(&md).unwrap_or_default();
            if !existing.contains(PIN_MARKER) {
                io.out(&format!("  SKIP: {} (non-pinned skill already exists)\n", skill_dir));
                return false;
            }
        }
    }
    let _ = std::fs::create_dir_all(&skill_dir);
    let _ = std::fs::write(jsp::join(&[&skill_dir, "SKILL.md"]), content);
    io.out(&format!("  + {}\n", skill_dir));
    true
}

/// JS: pin.mjs#removePinnedSkill
fn remove_pinned_skill(skills_dir: &str, command: &str, io: &mut Io) -> bool {
    let skill_dir = jsp::join(&[skills_dir, command]);
    if !exists(&skill_dir) {
        return false;
    }
    let md = jsp::join(&[&skill_dir, "SKILL.md"]);
    if !exists(&md) {
        return false;
    }
    let content = safe_read(&md).unwrap_or_default();
    if !content.contains(PIN_MARKER) {
        io.out(&format!("  SKIP: {} (not a pinned skill)\n", skill_dir));
        return false;
    }
    let _ = std::fs::remove_dir_all(&skill_dir);
    io.out(&format!("  - {}\n", skill_dir));
    true
}

pub fn run(args: &[String], io: &mut Io) -> i32 {
    let action = args.first().cloned();
    let command = args.get(1).cloned();
    let (Some(action), Some(command)) = (action.filter(|a| !a.is_empty()), command.filter(|c| !c.is_empty())) else {
        io.out("Usage: impeccable pin <pin|unpin> <command>\n");
        io.out(&format!("\nAvailable commands: {}\n", VALID_COMMANDS.join(", ")));
        return 1;
    };
    if action != "pin" && action != "unpin" {
        io.err(&format!("Unknown action: {}. Use 'pin' or 'unpin'.\n", action));
        return 1;
    }
    if !VALID_COMMANDS.contains(&command.as_str()) {
        io.err(&format!("Unknown command: {}\n", command));
        io.err(&format!("Available commands: {}\n", VALID_COMMANDS.join(", ")));
        return 1;
    }
    let cwd = io.cwd.to_string_lossy().into_owned();
    let root = find_project_root(&cwd);
    let provider = provider::detect(&io.env, &cwd);
    let running = running_target(&provider, io);
    if action == "pin" {
        let metadata = load_metadata(io);
        // The running install is the only one the agent asked about; without
        // one (source checkout), fall back to every install in the project.
        let targets = match &running {
            Some(target) => vec![target.clone()],
            None => scanned_targets(&root, io, false),
        };
        if targets.is_empty() {
            io.out("No harness directories with impeccable installed found.\n");
            return 0;
        }
        let mut created = 0;
        for target in &targets {
            let written = match target {
                Target::Skill { skills_dir } => write_pinned_skill(skills_dir, &command, &metadata, io),
                Target::Opencode { commands_dir } => write_pinned_opencode_command(commands_dir, &command, &metadata, io),
            };
            if written {
                created += 1;
            }
        }
        if created > 0 {
            io.out(&format!("\nPinned '{}' as a standalone shortcut in {} location(s).\n", command, created));
            io.out("Use the pinned command directly in each harness.\n");
        }
    } else {
        // Removal stays a sweep: every scope is marker-guarded, so cleanup can
        // reach a pin whose install has moved or disappeared since it was made.
        let mut targets = scanned_targets(&root, io, true);
        if let Some(target) = running {
            if !targets.iter().any(|t| t.key() == target.key()) {
                targets.push(target);
            }
        }
        let mut removed = 0;
        for target in &targets {
            let gone = match target {
                Target::Skill { skills_dir } => remove_pinned_skill(skills_dir, &command, io),
                Target::Opencode { commands_dir } => remove_pinned_opencode_command(commands_dir, &command, io),
            };
            if gone {
                removed += 1;
            }
        }
        if removed > 0 {
            io.out(&format!("\nUnpinned '{}' from {} location(s).\n", command, removed));
            io.out(&format!("Use Impeccable's '{}' workflow directly to access it.\n", command));
        } else {
            io.out(&format!("No pinned '{}' shortcut found.\n", command));
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::Env;

    fn scratch(tag: &str) -> String {
        let base = std::env::temp_dir().join(format!("impeccable-pin-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        base.to_string_lossy().into_owned()
    }

    fn put(path: &str, body: &str) {
        std::fs::create_dir_all(jsp::dirname(path)).unwrap();
        std::fs::write(path, body).unwrap();
    }

    fn env(home: &str, extra: &[(&str, &str)]) -> Env {
        let mut pairs: Vec<(String, String)> = vec![
            ("HOME".into(), home.into()),
            ("USERPROFILE".into(), home.into()),
        ];
        for (k, v) in extra {
            pairs.push(((*k).into(), (*v).into()));
        }
        Env::from_iter(pairs)
    }

    fn run_pin(args: &[&str], cwd: &str, env: Env) -> String {
        let (mut io, cap) = Io::captured("", std::path::PathBuf::from(cwd), env);
        let argv: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        run(&argv, &mut io);
        let out = String::from_utf8(cap.stdout.borrow().clone()).unwrap().replace('\\', "/");
        out
    }

    #[test]
    fn a_project_opencode_install_writes_the_slash_command() {
        let tmp = scratch("oc-project");
        let project = jsp::join(&[&tmp, "proj"]);
        std::fs::create_dir_all(jsp::join(&[&project, ".git"])).unwrap();
        put(&jsp::join(&[&project, ".opencode", "skills", "impeccable", "SKILL.md"]), "---\nname: impeccable\n---\n");

        let out = run_pin(
            &["pin", "polish"],
            &project,
            env(&tmp, &[("IMPECCABLE_SKILL_DIR", &jsp::join(&[&project, ".opencode", "skills", "impeccable"]))]),
        );

        let command = jsp::join(&[&project, ".opencode", "commands", "impeccable-polish.md"]);
        assert!(exists(&command), "{out}");
        assert!(safe_read(&command).unwrap_or_default().contains("<!-- impeccable-pinned-command -->"));
        assert!(!exists(&jsp::join(&[&project, ".opencode", "skills", "polish"])));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_user_opencode_install_writes_to_the_config_commands_dir() {
        let tmp = scratch("oc-user");
        let config = jsp::join(&[&tmp, "oc-config"]);
        let skill = jsp::join(&[&config, "skills", "impeccable"]);
        put(&jsp::join(&[&skill, "SKILL.md"]), "---\nname: impeccable\n---\n");

        let out = run_pin(
            &["pin", "polish"],
            &tmp,
            env(
                &tmp,
                &[
                    ("OPENCODE_CONFIG_DIR", &config),
                    ("IMPECCABLE_SKILL_DIR", &skill),
                    // The installed launcher cannot name this layout (its harness
                    // folder is `opencode`, not `.opencode`), so an OpenCode run
                    // names its provider; without it the scan below still finds
                    // the config install.
                    ("IMPECCABLE_PROVIDER_ID", "opencode"),
                ],
            ),
        );

        assert!(exists(&jsp::join(&[&config, "commands", "impeccable-polish.md"])), "{out}");
        assert!(!exists(&jsp::join(&[&config, "skills", "polish"])));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn the_running_install_is_the_only_one_pinned() {
        let tmp = scratch("running");
        let project = jsp::join(&[&tmp, "proj"]);
        std::fs::create_dir_all(jsp::join(&[&project, ".git"])).unwrap();
        // The project has a Claude install, but this run belongs to the global
        // .agents copy the agent actually loaded.
        put(&jsp::join(&[&project, ".claude", "skills", "impeccable", "SKILL.md"]), "---\nname: impeccable\n---\n");
        let skill = jsp::join(&[&tmp, ".agents", "skills", "impeccable"]);
        put(&jsp::join(&[&skill, "SKILL.md"]), "---\nname: impeccable\n---\n");

        let out = run_pin(&["pin", "polish"], &project, env(&tmp, &[("IMPECCABLE_SKILL_DIR", &skill)]));

        assert!(exists(&jsp::join(&[&tmp, ".agents", "skills", "polish", "SKILL.md"])), "{out}");
        assert!(!exists(&jsp::join(&[&project, ".claude", "skills", "polish"])));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_shared_agents_copy_keeps_the_skill_shortcut() {
        let tmp = scratch("agents");
        let skill = jsp::join(&[&tmp, ".agents", "skills", "impeccable"]);
        let io = {
            let (io, _cap) = Io::captured("", std::path::PathBuf::from(&tmp), env(&tmp, &[]));
            io
        };
        let target = target_for_skill_dir(&skill, &io);
        match target {
            Target::Skill { skills_dir } => assert_eq!(skills_dir, jsp::join(&[&tmp, ".agents", "skills"])),
            Target::Opencode { .. } => panic!(".agents is a skill surface, not OpenCode's"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_source_checkout_falls_back_to_the_project_scan() {
        let tmp = scratch("source");
        let project = jsp::join(&[&tmp, "proj"]);
        std::fs::create_dir_all(jsp::join(&[&project, ".git"])).unwrap();
        put(&jsp::join(&[&project, ".cursor", "skills", "impeccable", "SKILL.md"]), "---\nname: impeccable\n---\n");
        // A source checkout's provider id is "source": no harness folder above
        // the binary, so the running copy cannot name a destination.
        let source = jsp::join(&[&tmp, "checkout", "skill"]);
        put(&jsp::join(&[&source, "reference", "ios.md"]), "# ios\n");

        let out = run_pin(&["pin", "polish"], &project, env(&tmp, &[("IMPECCABLE_SKILL_DIR", &source)]));

        assert!(exists(&jsp::join(&[&project, ".cursor", "skills", "polish", "SKILL.md"])), "{out}");
        assert!(!exists(&jsp::join(&[&tmp, "checkout", "skill", "polish"])));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn unpin_reaches_a_global_running_pin() {
        let tmp = scratch("unpin");
        let project = jsp::join(&[&tmp, "proj"]);
        std::fs::create_dir_all(jsp::join(&[&project, ".git"])).unwrap();
        let skill = jsp::join(&[&tmp, ".agents", "skills", "impeccable"]);
        put(&jsp::join(&[&skill, "SKILL.md"]), "---\nname: impeccable\n---\n");

        run_pin(&["pin", "polish"], &project, env(&tmp, &[("IMPECCABLE_SKILL_DIR", &skill)]));
        let pinned = jsp::join(&[&tmp, ".agents", "skills", "polish", "SKILL.md"]);
        assert!(exists(&pinned));

        let out = run_pin(&["unpin", "polish"], &project, env(&tmp, &[("IMPECCABLE_SKILL_DIR", &skill)]));
        assert!(!exists(&pinned), "{out}");
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
