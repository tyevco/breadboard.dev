//! ADR-015 G5 (D6): a platform's content hash covers the `.repl` and the
//! plugin sources it declares (`bb_cpu_renode::platform_plugins`), so the
//! declaration must be complete. Against a real Renode 1.17.0, with its own
//! compiler (`AdHocCompiler`, the one the plugin cache uses):
//!
//! - each platform's declared subset compiles alone, as a separate
//!   assembly, so no declared model depends on a source the platform's hash
//!   does not cover;
//! - the whole assembly (every source, as the run-time cache compiles it)
//!   has no module initialiser, so a model a platform never instantiates
//!   cannot act on its runs (.NET runs a type's static initialiser only on
//!   first use);
//! - two negative controls show both checks act: a source that uses a type
//!   of another source does not compile alone, and a source with
//!   `[ModuleInitializer]` is found to have one.
//!
//! Not in `make test` (H4's budget is none; about 15 s): `make
//! plugin-scope`. Needs Renode; with `BB_REQUIRE_RENODE=1` a missing tool
//! fails the test instead of skipping.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bb_cpu_renode::console::{Console, COMMAND_ERROR};
use bb_cpu_renode::{find_renode, platform_plugins, PLATFORM_PLUGINS, PLUGIN_SOURCES};

const T: Duration = Duration::from_secs(180);

fn skip(msg: &str) {
    let required = std::env::var("BB_REQUIRE_RENODE").is_ok_and(|v| v == "1");
    assert!(!required, "BB_REQUIRE_RENODE=1 but {msg}");
    eprintln!("SKIP: {msg}");
}

/// Uses `BbStm32F0Adc` (bb_stm32f0_adc.cs): compiles only beside it.
const HIDDEN_DEP_CS: &str = r#"
using Antmicro.Renode.Peripherals.Analog;
namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public class BbScopeControlHiddenDep
    {
        public BbStm32F0Adc Adc;
    }
}
"#;

/// Has a module initialiser.
const MODULE_INIT_CS: &str = r#"
namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public static class BbScopeControlModuleInit
    {
        public static int Count;
        [System.Runtime.CompilerServices.ModuleInitializer]
        internal static void Init() { Count++; }
    }
}
"#;

/// The Renode-side script: compile each job, then report whether each
/// compiled assembly has a module initialiser (a `.cctor` on the `<Module>`
/// type, read from its metadata without loading it).
const SCRIPT_PY: &str = r#"
import System
import clr
clr.AddReference("System.Reflection.Metadata")
from System.Reflection.PortableExecutable import PEReader
from System.Reflection.Metadata import PEReaderExtensions
from Antmicro.Renode.Utilities import AdHocCompiler

def bb_has_module_initializer(dll):
    fs = System.IO.File.OpenRead(dll)
    try:
        pe = PEReader(fs)
        md = PEReaderExtensions.GetMetadataReader(pe)
        types = 0
        for th in md.TypeDefinitions:
            td = md.GetTypeDefinition(th)
            types += 1
            if md.GetString(td.Name) == "<Module>":
                for mh in td.GetMethods():
                    if md.GetString(md.GetMethodDefinition(mh).Name) == ".cctor":
                        return "yes", types
        return "no", types
    finally:
        fs.Dispose()

out = []
for line in System.IO.File.ReadAllLines(BB_JOBS):
    name, srcs = line.split("\t", 1)
    paths = srcs.split("\t")
    try:
        dll = AdHocCompiler().Compile(System.Array[System.String](paths))
        mi, types = bb_has_module_initializer(dll)
        out.append("OK\t%s\t%s\t%d" % (name, mi, types))
    except Exception as e:
        msg = str(e).replace("\r", " ").replace("\n", " | ").replace("\t", " ")
        out.append("FAIL\t%s\t%s" % (name, msg[:2000]))
System.IO.File.WriteAllLines(BB_RESULTS, System.Array[System.String](out))
"#;

#[derive(Debug)]
enum Res {
    Ok { module_init: bool, types: u32 },
    Fail(String),
}

#[test]
#[ignore = "about 15 s of Renode compiles; run by `make plugin-scope`"]
fn each_platform_subset_compiles_alone_and_no_module_initialiser() {
    let Some(renode) = find_renode() else {
        return skip("renode not found (set BB_RENODE or run `make setup`)");
    };
    let dir = std::env::temp_dir().join(format!("bb-plugin-scope-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = |n: &str| -> PathBuf { dir.join(n) };
    for (n, s) in PLUGIN_SOURCES {
        std::fs::write(path(n), s).unwrap();
    }
    std::fs::write(path("bb_scope_hidden_dep.cs"), HIDDEN_DEP_CS).unwrap();
    std::fs::write(path("bb_scope_module_init.cs"), MODULE_INIT_CS).unwrap();

    // The jobs: every platform's declared subset, the whole assembly, and
    // the two controls.
    let mut jobs: Vec<(String, Vec<String>)> = Vec::new();
    for (repl, _) in PLATFORM_PLUGINS {
        let srcs = platform_plugins(repl).unwrap();
        jobs.push((
            format!("subset:{repl}"),
            srcs.iter().map(|s| s.to_string()).collect(),
        ));
    }
    jobs.push((
        "all".into(),
        PLUGIN_SOURCES.iter().map(|(n, _)| n.to_string()).collect(),
    ));
    jobs.push((
        "control:hidden-dep".into(),
        vec!["bb_scope_hidden_dep.cs".into()],
    ));
    jobs.push((
        "control:hidden-dep-declared".into(),
        vec!["bb_scope_hidden_dep.cs".into(), "bb_stm32f0_adc.cs".into()],
    ));
    jobs.push((
        "control:module-init".into(),
        vec!["bb_scope_module_init.cs".into()],
    ));
    let lines: Vec<String> = jobs
        .iter()
        .map(|(n, srcs)| {
            let p: Vec<String> = srcs.iter().map(|s| path(s).display().to_string()).collect();
            format!("{n}\t{}", p.join("\t"))
        })
        .collect();
    let jobs_file = path("jobs.tsv");
    let results_file = path("results.tsv");
    std::fs::write(&jobs_file, lines.join("\n") + "\n").unwrap();
    std::fs::write(
        path("scope.py"),
        format!(
            "BB_JOBS = r\"{}\"\nBB_RESULTS = r\"{}\"\n{SCRIPT_PY}",
            jobs_file.display(),
            results_file.display()
        ),
    )
    .unwrap();

    let t0 = Instant::now();
    let mut con = Console::spawn(&renode, &dir, T).expect("renode console");
    let out = con
        .run(&[&format!("include @{}", path("scope.py").display())], T)
        .expect("the scope script ran");
    con.kill();
    eprintln!(
        "plugin scope: {} jobs in {:.1} s",
        jobs.len(),
        t0.elapsed().as_secs_f64()
    );
    assert!(!out.contains(COMMAND_ERROR), "{out}");
    let text = std::fs::read_to_string(&results_file)
        .unwrap_or_else(|e| panic!("no results ({e}); Renode printed:\n{out}"));
    let mut res = std::collections::BTreeMap::new();
    for l in text.lines() {
        let f: Vec<&str> = l.split('\t').collect();
        let r = match f[0] {
            "OK" => Res::Ok {
                module_init: f[2] == "yes",
                types: f[3].parse().unwrap(),
            },
            _ => Res::Fail(f[2..].join(" ")),
        };
        eprintln!("  {}: {r:?}", f[1]);
        res.insert(f[1].to_string(), r);
    }
    assert_eq!(res.len(), jobs.len(), "{text}");

    // Every platform's subset, and the whole assembly, compile and have no
    // module initialiser.
    for (repl, _) in PLATFORM_PLUGINS {
        match &res[&format!("subset:{repl}")] {
            Res::Ok { module_init, types } => {
                assert!(!module_init, "{repl}: a module initialiser");
                assert!(*types > 1, "{repl}: an empty assembly");
            }
            Res::Fail(e) => panic!("{repl}'s declared plugin subset does not compile alone: {e}"),
        }
    }
    match &res["all"] {
        Res::Ok { module_init, .. } => {
            assert!(!module_init, "the plugin assembly has a module initialiser")
        }
        Res::Fail(e) => panic!("the plugin assembly does not compile: {e}"),
    }

    // The controls: a hidden dependency fails alone and compiles declared;
    // a module initialiser is found.
    match &res["control:hidden-dep"] {
        Res::Fail(e) => assert!(e.contains("BbStm32F0Adc"), "{e}"),
        r => panic!("the hidden-dependency control compiled alone: {r:?}"),
    }
    assert!(matches!(
        res["control:hidden-dep-declared"],
        Res::Ok {
            module_init: false,
            ..
        }
    ));
    assert!(
        matches!(
            res["control:module-init"],
            Res::Ok {
                module_init: true,
                ..
            }
        ),
        "the module-initialiser control was not detected: {:?}",
        res["control:module-init"]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
