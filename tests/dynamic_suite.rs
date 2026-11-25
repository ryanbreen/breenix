//! Check the guest case order and the artifact receipt produced by KD staging.

use serde_json::Value;
use std::{fs, path::Path, process::Command};
use syn::{Expr, ExprCall, Item};

fn call<'a>(expr: &'a Expr, name: &str, arity: usize) -> &'a ExprCall {
    let Expr::Call(call) = expr else {
        panic!("expected {name}(...)");
    };
    assert!(matches!(&*call.func, Expr::Path(path) if path.path.is_ident(name)));
    assert_eq!(call.args.len(), arity, "{name}");
    call
}

fn entry(call: &ExprCall) -> (String, String) {
    let literal = |index| match &call.args[index] {
        Expr::Lit(lit) if lit.attrs.is_empty() => match &lit.lit {
            syn::Lit::Str(value) => value.value(),
            _ => panic!("registration field {index} must be a string"),
        },
        _ => panic!("registration field {index} must be a string"),
    };
    (literal(0), literal(1))
}

fn slice(expr: &Expr) -> &syn::ExprArray {
    let Expr::Reference(reference) = expr else {
        panic!("expected a borrowed registration array");
    };
    let Expr::Array(array) = &*reference.expr else {
        panic!("expected a literal registration array");
    };
    array
}

fn manifest_entry(value: &Value) -> (String, String) {
    (
        value["id"].as_str().unwrap().into(),
        value["title"].as_str().unwrap().into(),
    )
}

#[test]
fn manifest_matches_guest_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: Value = serde_json::from_slice(
        &fs::read(root.join("docs/suites/dynamic.json")).expect("dynamic suite manifest"),
    )
    .unwrap();
    assert_eq!(manifest["binary"], "/sbin/suite-dynamic");
    assert_eq!(manifest["id"], "dynamic");

    let source = fs::read_to_string(root.join("userspace/programs/src/suite_dynamic.rs"))
        .expect("dynamic suite source");
    let file = syn::parse_file(&source).unwrap();
    let main = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Fn(main) if main.sig.ident == "main" => Some(main),
            _ => None,
        })
        .expect("guest main");
    let [syn::Stmt::Expr(Expr::MethodCall(run), _)] = main.block.stmts.as_slice() else {
        panic!("guest main must run its registered suite directly");
    };
    assert_eq!(run.method, "run");
    assert!(run.args.is_empty());
    let Expr::Path(table_name) = &*run.receiver else {
        panic!("guest main must use a static registration");
    };
    let table = file
        .items
        .iter()
        .find_map(|item| match item {
            Item::Static(table) if table_name.path.is_ident(&table.ident) => Some(table),
            _ => None,
        })
        .expect("the guest table main runs");
    let suite = call(&table.expr, "suite", 3);
    assert_eq!(entry(suite), manifest_entry(&manifest));
    let [category] = manifest["categories"].as_array().unwrap().as_slice() else {
        panic!("KD has one category");
    };
    assert_eq!(category["id"], "dynamic");
    let categories = &slice(&suite.args[2]).elems;
    assert_eq!(categories.len(), 1);
    let guest_category = call(&categories[0], "category", 3);
    assert_eq!(entry(guest_category), manifest_entry(category));

    let expected = [
        "interp-runs-program",
        "missing-interp-fails",
        "dso-relocs-and-tls",
    ];
    let cases = category["cases"].as_array().unwrap();
    assert_eq!(
        cases
            .iter()
            .map(|case| case["id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        expected
    );
    let guest_cases = &slice(&guest_category.args[2]).elems;
    assert_eq!(guest_cases.len(), expected.len());
    for (guest, manifest) in guest_cases.iter().zip(cases) {
        assert_eq!(entry(call(guest, "case", 3)), manifest_entry(manifest));
    }
}

#[test]
fn staged_digests_match_build_record() {
    let output = Command::new("python3")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args([
            "-c",
            r#"
import hashlib
import json
import os
from pathlib import Path
import runpy
import subprocess
import sys
import tempfile

stage = Path('scripts/rust_host_stage.py').resolve()
tools = runpy.run_path(str(stage))['require_e2fsprogs']
mke2fs, debugfs = tools('mke2fs', 'debugfs')
prefix = '/usr/libexec/breenix/dynamic'
paths = {
    '/sbin/suite-dynamic': 'suite-dynamic',
    '/lib/ld-breenix.so': 'ld-breenix.so',
    '/lib/libc.so': 'libc.so',
    f'{prefix}/dynamic-probe': 'dynamic-probe',
    f'{prefix}/missing-interp': 'missing-interp',
    f'{prefix}/libkd-probe.so': 'libkd-probe.so',
}

def digest(data):
    return hashlib.sha256(data).hexdigest()

with tempfile.TemporaryDirectory(prefix='dynamic-suite-check-') as temporary:
    root = Path(temporary)
    artifacts = Path(os.environ.get('BREENIX_DYNAMIC_TEST_ARTIFACTS', root))
    if artifacts == root:
        # Distinct inputs test copying and receipt routing without a guest toolchain.
        for guest, name in paths.items():
            (artifacts / name).write_bytes(b'\x7fELF' + guest.encode())
    image = root / 'ext2.img'
    subprocess.run([mke2fs, '-q', '-t', 'ext2', '-F', image, '16384'],
                   check=True, capture_output=True, timeout=5)
    command = [sys.executable, stage, '--arch', 'x86_64', '--image', image, '--suite']
    env = dict(os.environ, BREENIX_KD_BUILD_DIR=str(artifacts.resolve()))
    before = digest(image.read_bytes())
    excluded = subprocess.check_output([*command, 'ipc,dynamic-extra'], env=env, timeout=5)
    assert not excluded and digest(image.read_bytes()) == before, 'ordinary root changed'
    # metal-build records the stager's verified stdout as build.json stagedFiles.
    receipt = json.loads(subprocess.check_output([*command, 'dynamic'], env=env, timeout=15))
    assert set(receipt) == set(paths), (sorted(receipt), sorted(paths))
    for guest, name in paths.items():
        data = subprocess.check_output([debugfs, '-R', f'cat {guest}', image],
                                       stderr=subprocess.PIPE, timeout=3)
        assert receipt[guest] == digest(data) == digest((artifacts / name).read_bytes()), guest
        stat = subprocess.check_output([debugfs, '-R', f'stat {guest}', image],
                                       stderr=subprocess.PIPE, text=True, timeout=3)
        assert 'Type: regular    Mode:  0755' in stat, (guest, stat)
    absent = subprocess.check_output(
        [debugfs, '-R', 'cat /lib/ld-breenix-kd-missing.so', image],
        stderr=subprocess.PIPE, timeout=3,
    )
    assert not absent, 'the negative interpreter was staged'
"#,
        ])
        .output()
        .expect("run dynamic staging check");
    assert!(
        output.status.success(),
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
