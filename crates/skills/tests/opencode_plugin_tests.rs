//! The OpenCode design-hook plugin install: OpenCode hooks are in-process
//! plugin callbacks, so the bundle ships a JavaScript plugin instead of a
//! command-hook manifest, and the destination differs by scope — a project
//! reads `<project>/.opencode/plugins/`, a user install reads the global
//! config dir (never `<home>/.opencode/`, #406).

use impeccable_common::jsp;
use impeccable_skills::hook_manifest::{copy_provider_hooks, hook_installed_for_provider, hook_installed_for_provider_at_scope};
use impeccable_skills::providers::Sys;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const PLUGIN_REL: &str = ".opencode/plugins/impeccable-design-hook.js";
const PLUGIN_BODY: &str = "// impeccable-design-hook: the Impeccable design detector as an OpenCode plugin.\nexport default { id: \"impeccable-design-hook\" }\n";

struct Fixture {
    root: PathBuf,
    bundle: String,
    home: String,
    project: String,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let root = std::env::temp_dir().join(format!("imp-opencode-{name}-{}-{nanos}", std::process::id()));
        let bundle = root.join("bundle");
        std::fs::create_dir_all(bundle.join(".opencode/plugins")).unwrap();
        std::fs::write(bundle.join(PLUGIN_REL), PLUGIN_BODY).unwrap();
        std::fs::create_dir_all(root.join("home")).unwrap();
        std::fs::create_dir_all(root.join("project")).unwrap();
        let s = |p: PathBuf| p.to_string_lossy().into_owned();
        Fixture { bundle: s(bundle), home: s(root.join("home")), project: s(root.join("project")), root }
    }

    fn sys(&self) -> Sys {
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), self.home.clone());
        env.insert("USERPROFILE".to_string(), self.home.clone());
        Sys::new(env, self.project.clone())
    }

    fn install(&self, sys: &Sys, root: &str) -> Result<Vec<String>, String> {
        copy_provider_hooks(sys, &self.bundle, root, &[".opencode"], false, None, None)
    }

    fn project_plugin(&self) -> String {
        jsp::join(&[&self.project, ".opencode", "plugins", "impeccable-design-hook.js"])
    }

    fn user_plugin(&self) -> String {
        jsp::join(&[&self.home, ".config", "opencode", "plugins", "impeccable-design-hook.js"])
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn project_install_writes_the_plugin_where_opencode_reads_it() {
    let f = Fixture::new("project");
    let sys = f.sys();
    assert!(!hook_installed_for_provider(&sys, &f.project, ".opencode"));
    assert_eq!(f.install(&sys, &f.project).unwrap(), vec![f.project_plugin()]);
    assert_eq!(std::fs::read_to_string(f.project_plugin()).unwrap(), PLUGIN_BODY);
    assert!(hook_installed_for_provider(&sys, &f.project, ".opencode"));
    // Idempotent: a repeat install reports the same path and leaves the file alone.
    assert_eq!(f.install(&sys, &f.project).unwrap(), vec![f.project_plugin()]);
    assert_eq!(std::fs::read_to_string(f.project_plugin()).unwrap(), PLUGIN_BODY);
}

#[test]
fn user_install_uses_the_global_config_dir_not_a_dot_opencode_in_home() {
    let f = Fixture::new("user");
    let sys = f.sys();
    assert!(sys.is_home_dir(&f.home));
    assert_eq!(f.install(&sys, &f.home).unwrap(), vec![f.user_plugin()]);
    assert!(Path::new(&f.user_plugin()).is_file());
    assert!(!Path::new(&f.home).join(".opencode").exists(), "OpenCode reads <config>/plugins/, not <home>/.opencode/");
    assert!(hook_installed_for_provider_at_scope(&sys, &f.home, ".opencode", true));
    // A project-scope check must not read the user-level copy.
    assert!(!hook_installed_for_provider(&sys, &f.home, ".opencode"));
}

#[test]
fn opencode_config_dir_overrides_the_user_destination() {
    let f = Fixture::new("configdir");
    let custom = f.root.join("custom-opencode").to_string_lossy().into_owned();
    let mut env = HashMap::new();
    env.insert("HOME".to_string(), f.home.clone());
    env.insert("OPENCODE_CONFIG_DIR".to_string(), custom.clone());
    let sys = Sys::new(env, f.project.clone());
    let dest = jsp::join(&[&custom, "plugins", "impeccable-design-hook.js"]);
    assert_eq!(f.install(&sys, &f.home).unwrap(), vec![dest.clone()]);
    assert!(Path::new(&dest).is_file());
    assert!(!Path::new(&f.user_plugin()).exists());
}

#[test]
fn a_foreign_file_at_the_destination_is_replaced_not_counted() {
    let f = Fixture::new("foreign");
    let sys = f.sys();
    let dest = PathBuf::from(f.project_plugin());
    std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
    std::fs::write(&dest, "// somebody else's plugin\nexport default {}\n").unwrap();
    assert!(!hook_installed_for_provider(&sys, &f.project, ".opencode"));
    f.install(&sys, &f.project).unwrap();
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), PLUGIN_BODY);
    assert!(hook_installed_for_provider(&sys, &f.project, ".opencode"));
}

#[test]
fn a_bundle_without_the_plugin_writes_nothing() {
    let f = Fixture::new("absent");
    std::fs::remove_file(Path::new(&f.bundle).join(PLUGIN_REL)).unwrap();
    let sys = f.sys();
    assert_eq!(f.install(&sys, &f.project).unwrap(), Vec::<String>::new());
    assert!(!Path::new(&f.project_plugin()).exists());
    assert!(!hook_installed_for_provider(&sys, &f.project, ".opencode"));
}

/// Runs `install --providers=opencode` against a staged bundle zip and returns
/// its stdout, so the provider wiring (not just the copy helper) is exercised.
#[cfg(unix)]
fn run_opencode_install(f: &Fixture, with_plugin: bool) -> String {
    use std::io::Write as _;
    let (os, arch) = impeccable_skills::engine_binary::platform_tag().expect("supported platform");
    std::fs::create_dir_all(Path::new(&f.project).join(".git")).unwrap();
    let zip_path = f.root.join(if with_plugin { "bundle.zip" } else { "bundle-old.zip" });
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    let plain = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let exec = plain.unix_permissions(0o755);
    let mut entries = vec![
        (".opencode/skills/impeccable/SKILL.md".to_string(), "---\nname: impeccable\n---\n", plain),
        (".opencode/skills/impeccable/scripts/impeccable".to_string(), "#!/bin/sh\nexit 0\n", exec),
        (".opencode/skills/impeccable/scripts/VERSION".to_string(), "9.9.9\n", plain),
        (format!(".opencode/skills/impeccable/scripts/bin/{os}-{arch}/impeccable"), "#!/bin/sh\nexit 0\n", exec),
        (".opencode/commands/impeccable.md".to_string(), "---\ndescription: x\n---\n", plain),
    ];
    if with_plugin {
        entries.push((PLUGIN_REL.to_string(), PLUGIN_BODY, plain));
    }
    for (name, body, opts) in entries {
        zip.start_file(name, opts).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    zip.finish().unwrap();

    let mut env = HashMap::new();
    env.insert("HOME".to_string(), f.home.clone());
    env.insert("TMPDIR".to_string(), f.root.join("tmp").to_string_lossy().into_owned());
    env.insert("IMPECCABLE_BUNDLE_PATH".to_string(), zip_path.to_string_lossy().into_owned());
    std::fs::create_dir_all(f.root.join("tmp")).unwrap();
    let (mut io, captured) = impeccable_common::Io::captured("", PathBuf::from(&f.project), env);
    let args = ["install", "-y", "--providers=opencode", "--scope=project"].map(String::from);
    let code = impeccable_skills::run(&args, &mut io);
    let stdout = String::from_utf8_lossy(&captured.stdout.borrow()).into_owned();
    assert_eq!(code, 0, "install failed:\n{stdout}\n{}", String::from_utf8_lossy(&captured.stderr.borrow()));
    stdout
}

/// The install says the plugin needs a reload, once: a repeat install writes
/// nothing, so it must not repeat the instruction.
#[cfg(unix)]
#[test]
fn install_names_the_reload_step_once() {
    if impeccable_skills::engine_binary::platform_tag().is_none() {
        return;
    }
    const RELOAD: &str = "restart it, or run /reload";
    let f = Fixture::new("reload");
    assert!(run_opencode_install(&f, true).contains(RELOAD), "first install names the step");
    assert!(Path::new(&f.project_plugin()).is_file());
    let repeat = run_opencode_install(&f, true);
    assert!(!repeat.contains(RELOAD), "repeat install writes nothing to reload:\n{repeat}");
    assert!(!repeat.contains("Installed hooks into"), "{repeat}");
}

/// A bundle from before the plugin installs the skill and no plugin, so there
/// is nothing to reload and nothing is claimed.
#[cfg(unix)]
#[test]
fn a_bundle_without_the_plugin_names_no_reload_step() {
    if impeccable_skills::engine_binary::platform_tag().is_none() {
        return;
    }
    let f = Fixture::new("noreload");
    let out = run_opencode_install(&f, false);
    assert!(!out.contains("/reload"), "{out}");
    assert!(!Path::new(&f.project_plugin()).exists());
}
