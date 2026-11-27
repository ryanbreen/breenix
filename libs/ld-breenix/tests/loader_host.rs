use ld_breenix::{Dependency, ErrorKind, InitKind, InitState, Object, Patch, Scope};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

const NAMES: [&[u8]; 3] = [
    b"/fixture/fixture",
    b"/fixture/lib/libouter.so",
    b"/fixture/lib/private/libinner.so",
];
const FILES: [&str; 3] = ["fixture", "lib/libouter.so", "lib/private/libinner.so"];
const BIASES: [u64; 3] = [0x4000_0000, 0x5000_0000, 0x6000_0000];

struct Fixture {
    file: Vec<u8>,
    readelf: String,
    symbols: BTreeMap<String, u64>,
}

fn hex(text: &str) -> u64 {
    u64::from_str_radix(text.trim_start_matches("0x"), 16).unwrap()
}

#[test]
fn ld_breenix_is_self_contained_dyn() {
    let path = std::env::var_os("BREENIX_LD_BREENIX")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../userspace/programs/target/kd/ld-breenix.so")
        });
    assert!(
        path.is_file(),
        "build the interpreter first: {}",
        path.display()
    );
    let inspect = |setting: &str, fallback: &str, args: &[&str]| {
        let tool = std::env::var_os(setting).unwrap_or_else(|| fallback.into());
        let result = Command::new(tool).args(args).arg(&path).output().unwrap();
        assert!(result.status.success(), "{result:?}");
        String::from_utf8(result.stdout).unwrap()
    };
    let elf = inspect(
        "BREENIX_TOOLCHAIN_READELF",
        "llvm-readelf",
        &[
            "--wide",
            "--file-header",
            "--program-headers",
            "--dynamic-table",
            "--dyn-relocations",
        ],
    );
    let field = |tag| {
        elf.lines()
            .find_map(|line| line.trim().strip_prefix(tag))
            .unwrap()
            .trim()
    };
    assert_eq!(field("Type:").split_whitespace().next(), Some("DYN"));
    assert_eq!(field("Machine:"), "Advanced Micro Devices X86-64");
    let entry = hex(field("Entry point address:"));
    assert!(!elf
        .lines()
        .any(|line| line.trim_start().starts_with("INTERP ")));
    for kind in ["NEEDED", "REL", "RELR", "JMPREL"] {
        assert!(
            !elf.contains(&format!("({kind})")),
            "{kind} in the interpreter"
        );
    }
    for relocation in elf
        .lines()
        .filter_map(|line| line.split_whitespace().nth(2))
    {
        if relocation.starts_with("R_") {
            assert_eq!(relocation, "R_X86_64_RELATIVE");
        }
    }
    assert!(
        elf.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.first() == Some(&"LOAD")
                && fields[6..fields.len() - 1].contains(&"E")
                && (hex(fields[2])..hex(fields[2]) + hex(fields[5])).contains(&entry)
        }),
        "entry {entry:#x} is outside every executable PT_LOAD"
    );
    let undefined = inspect("BREENIX_TOOLCHAIN_NM", "llvm-nm", &["--undefined-only"]);
    assert!(undefined.trim().is_empty(), "{undefined}");
}

impl Fixture {
    fn file_offset(&self, segment: &str) -> usize {
        let mut fields = self
            .readelf
            .lines()
            .find_map(|line| line.trim_start().strip_prefix(segment))
            .unwrap()
            .split_whitespace();
        hex(fields.next().unwrap()) as usize
    }

    fn relro(&self, bias: u64) -> std::ops::Range<u64> {
        let fields: Vec<_> = self
            .readelf
            .lines()
            .find(|line| line.trim_start().starts_with("GNU_RELRO "))
            .unwrap()
            .split_whitespace()
            .collect();
        let start = bias + hex(fields[2]);
        start..start + hex(fields[5])
    }

    fn first_rela(&self) -> usize {
        let heading = self
            .readelf
            .lines()
            .find(|line| line.starts_with("Relocation section '.rela.dyn'"))
            .unwrap();
        hex(heading.split_whitespace().nth(5).unwrap()) as usize
    }
}

fn fixtures() -> &'static [Fixture; 4] {
    static FIXTURES: OnceLock<[Fixture; 4]> = OnceLock::new();
    FIXTURES.get_or_init(|| build_fixtures("x86_64", [FILES[0], FILES[1], FILES[2], "libtls.so"]))
}

fn build_fixtures<const N: usize>(arch: &str, names: [&str; N]) -> [Fixture; N] {
    let script =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/dynamic/build.sh");
    let temp = Command::new("mktemp").arg("-d").output().unwrap();
    assert!(temp.status.success(), "{temp:?}");
    struct Scratch(PathBuf);
    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove the fixture directory");
        }
    }
    let temp = Scratch(PathBuf::from(
        std::str::from_utf8(&temp.stdout).unwrap().trim(),
    ));
    let result = Command::new(script)
        .arg(&temp.0)
        .arg(arch)
        .output()
        .unwrap();
    assert!(result.status.success(), "{result:?}");
    names.map(|name| Fixture {
        file: fs::read(temp.0.join(name)).unwrap(),
        readelf: fs::read_to_string(temp.0.join(format!("{name}.readelf"))).unwrap(),
        symbols: fs::read_to_string(temp.0.join(format!("{name}.nm")))
            .unwrap()
            .lines()
            .map(|line| {
                let mut fields = line.split_whitespace();
                let symbol = fields.next().unwrap().to_owned();
                (symbol, hex(fields.nth(1).unwrap()))
            })
            .collect(),
    })
}

#[test]
fn executable_relocates_from_mapped_metadata() {
    let [root, outer, inner, _] = fixtures();
    let mut mapped = vec![0u8; 0x10000];
    let mut disk = root.file.clone();
    let get = |offset| u64::from_le_bytes(root.file[offset..offset + 8].try_into().unwrap());
    let phoff = get(32) as usize;
    let phnum = u16::from_le_bytes(root.file[56..58].try_into().unwrap()) as usize;
    let mut phdr = None;
    for offset in (phoff..phoff + phnum * 56).step_by(56) {
        if root.file[offset..offset + 4] == 1u32.to_le_bytes() {
            let src = get(offset + 8) as usize;
            let dst = get(offset + 16) as usize;
            let len = get(offset + 32) as usize;
            mapped[dst..dst + len].copy_from_slice(&disk[src..src + len]);
            if src <= phoff && phoff + phnum * 56 <= src + len {
                phdr = Some(dst + phoff - src);
            }
        }
    }
    // The pathname's current file has different dynamic metadata. The loader's
    // exec input consists only of the original PHDRs and load segments.
    disk[root.file_offset("DYNAMIC")..].fill(0);
    let bias = mapped.as_mut_ptr() as u64;
    let phdr = phdr.unwrap();
    let snapshot = |bytes: &[u8]| -> Option<&'static [u8]> {
        Some(Box::leak(bytes.to_vec().into_boxed_slice()))
    };
    // SAFETY: the fixture's PT_LOAD ranges point into `mapped`.
    let main = unsafe {
        Object::from_mapped(
            NAMES[0],
            &mapped[phdr..phdr + phnum * 56],
            bias,
            bias + get(24),
            snapshot,
        )
    }
    .unwrap();
    mapped.fill(0);
    assert!(Object::parse(NAMES[0], &disk, bias).is_err());
    let scope = [
        main,
        Object::parse(NAMES[1], &outer.file, BIASES[1]).unwrap(),
        Object::parse(NAMES[2], &inner.file, BIASES[2]).unwrap(),
    ];
    let mut patches = Vec::new();
    main.relocate(&scope, |patch| patches.push(patch)).unwrap();
    let from_file = Object::parse(NAMES[0], &root.file, bias).unwrap();
    let mut expected = Vec::new();
    from_file
        .relocate(&[from_file, scope[1], scope[2]], |patch| expected.push(patch))
        .unwrap();
    assert!(!expected.is_empty());
    assert_eq!(patches, expected);
}

#[test]
fn dtpmod64_gets_module_id() {
    let fixtures = fixtures();
    let root = Object::parse(NAMES[0], &fixtures[0].file, BIASES[0]).unwrap();
    let tls = &fixtures[3];
    let tls_header: Vec<_> = tls
        .readelf
        .lines()
        .find(|line| line.trim_start().starts_with("TLS "))
        .unwrap()
        .split_whitespace()
        .collect();
    assert_eq!(
        (hex(tls_header[4]), hex(tls_header[5]), hex(tls_header[7])),
        (0, 48, 8)
    );
    let mut slots = tls.readelf.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let address = fields.next()?;
        let info = fields.next()?;
        (fields.next()? == "R_X86_64_DTPMOD64").then(|| {
            assert_eq!(hex(info) >> 32, 0);
            hex(address)
        })
    });
    let slot = slots
        .next()
        .expect("fixture must use a local-dynamic module slot");
    assert_eq!(slots.next(), None);
    let first = Object::parse(b"/fixture/first.so", &tls.file, BIASES[1]).unwrap();
    let second = Object::parse(b"/fixture/second.so", &tls.file, BIASES[2]).unwrap();
    let libc_path = std::env::var_os("BREENIX_KD_LIBC")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../userspace/programs/target/kd/libc.so")
        });
    let libc_file = fs::read(&libc_path).unwrap_or_else(|error| {
        panic!("build hosted libc first: {}: {error}", libc_path.display())
    });
    let libc = Object::parse(b"/lib/libc.so", &libc_file, 0x7000_0000).unwrap();
    assert!(libc.lookup(b"__tls_get_addr").unwrap().is_some());
    let scope = [root, first, libc, second];
    for (object, id) in [(first, 1), (second, 2)] {
        let mut module = None;
        object
            .relocate(&scope, |patch| {
                if patch.address == object.bias() + slot {
                    assert!(module.replace(patch.value).is_none());
                }
            })
            .unwrap();
        assert_eq!(module, Some(id));
    }
}

#[test]
fn two_dso_fixture_resolves_every_value() {
    let fixtures = fixtures();
    let objects = std::array::from_fn::<_, 3, _>(|i| {
        Object::parse(NAMES[i], &fixtures[i].file, BIASES[i]).unwrap()
    });
    let mut queue = [objects[0]; 3];
    let mut edges = [Dependency::default(); 3];
    let mut path = [0; 128];
    let mut attempts = Vec::new();
    let scope = Scope::load(objects[0], &mut queue, &mut edges, &mut path, |path| {
        attempts.push(path.to_vec());
        Ok(objects[1..]
            .iter()
            .rev()
            .copied()
            .find(|obj| obj.name() == path))
    })
    .unwrap();
    assert_eq!(
        attempts,
        [
            b"/fixture/missing/libouter.so".as_slice(),
            NAMES[1],
            NAMES[2]
        ]
    );
    assert_eq!(
        scope
            .objects()
            .iter()
            .map(|obj| obj.name())
            .collect::<Vec<_>>(),
        NAMES
    );

    let mut seen_kinds = BTreeSet::new();
    let mut relocated = [const { BTreeMap::new() }; 3];
    for (i, obj) in scope.objects().iter().enumerate() {
        let mut expected = BTreeMap::new();
        for line in fixtures[i].readelf.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            let Some(kind) = fields.get(2).filter(|kind| kind.starts_with("R_X86_64_")) else {
                continue;
            };
            seen_kinds.insert(*kind);
            let address = BIASES[i] + hex(fields[0]);
            let value = match *kind {
                "R_X86_64_RELATIVE" => BIASES[i] + hex(fields[3]),
                "R_X86_64_64" | "R_X86_64_GLOB_DAT" | "R_X86_64_JUMP_SLOT" => {
                    // The fixture's global definitions have default visibility.
                    // llvm-nm supplies each candidate's link-time symbol value.
                    let symbol = fields[4];
                    let resolved = fixtures
                        .iter()
                        .zip(BIASES)
                        .find_map(|(file, bias)| file.symbols.get(symbol).map(|value| bias + value))
                        .unwrap_or_else(|| {
                            assert_eq!(symbol, "missing_weak");
                            0
                        });
                    if *kind == "R_X86_64_64" {
                        assert_eq!(fields[5], "+");
                        resolved + hex(fields[6])
                    } else {
                        resolved
                    }
                }
                _ => panic!("unexpected fixture relocation {kind}"),
            };
            assert!(expected.insert(address, value).is_none());
        }
        assert!(!expected.is_empty());
        obj.relocate(scope.objects(), |Patch { address, value }| {
            assert!(relocated[i].insert(address, value).is_none());
        })
        .unwrap();
        assert_eq!(relocated[i], expected, "{:?}", FILES[i]);
        assert_eq!(obj.relro(), Some(fixtures[i].relro(BIASES[i])));
    }
    assert_eq!(
        seen_kinds,
        BTreeSet::from([
            "R_X86_64_64",
            "R_X86_64_GLOB_DAT",
            "R_X86_64_JUMP_SLOT",
            "R_X86_64_RELATIVE",
        ])
    );

    let mut observed = Vec::new();
    scope
        .initializers(&mut [InitState::default(); 3], |array| {
            for slot in array.range.step_by(8) {
                observed.push((array.object, array.kind, relocated[array.object][&slot]));
            }
        })
        .unwrap();
    let at = |i: usize, name: &str| BIASES[i] + fixtures[i].symbols[name];
    assert_eq!(
        observed,
        [
            (0, InitKind::Preinit, at(0, "preinit_first")),
            (0, InitKind::Preinit, at(0, "preinit_second")),
            (2, InitKind::Init, at(2, "inner_init")),
            (1, InitKind::Init, at(1, "outer_init")),
            (0, InitKind::Init, at(0, "main_init")),
        ]
    );
}

#[test]
fn relocate_is_error_not_panic() {
    let fixtures = fixtures();
    let outer = Object::parse(NAMES[1], &fixtures[1].file, BIASES[1]).unwrap();
    let missing = outer.relocate(&[outer], |_| {}).unwrap_err();
    assert_eq!(missing.kind, ErrorKind::UndefinedSymbol);
    assert_eq!(missing.object, NAMES[1]);
    assert_eq!(missing.detail, b"inner_data");
    assert_eq!(missing.offset, fixtures[1].first_rela() as u64 + 24);

    let fixture = &fixtures[0];
    let truncated = &fixture.file[..fixture.file_offset("DYNAMIC ") + 7];
    assert_eq!(
        Object::parse(NAMES[0], truncated, BIASES[0])
            .unwrap_err()
            .kind,
        ErrorKind::Bounds
    );

    let offset = fixture.first_rela();
    let mut corrupt = fixture.file.clone();
    corrupt[offset + 8..offset + 12].copy_from_slice(&u32::MAX.to_le_bytes());
    let obj = Object::parse(NAMES[0], &corrupt, BIASES[0]).unwrap();
    let bad_type = obj
        .relocate(&[obj], |_| panic!("invalid first write"))
        .unwrap_err();
    assert_eq!(bad_type.offset, offset as u64);
    assert_eq!(bad_type.kind, ErrorKind::UnsupportedRelocation(u32::MAX));

    corrupt[offset + 8..offset + 12].copy_from_slice(&8u32.to_le_bytes());
    corrupt[offset..offset + 8].copy_from_slice(&u64::MAX.to_le_bytes());
    let obj = Object::parse(NAMES[0], &corrupt, BIASES[0]).unwrap();
    let bad_slot = obj
        .relocate(&[obj], |_| panic!("out-of-bounds first write"))
        .unwrap_err();
    assert_eq!(bad_slot.offset, offset as u64);
    assert_eq!(bad_slot.kind, ErrorKind::Bounds);
}

mod libc_host {
    use super::*;

    #[test]
    fn dlsym_follows_scope_order() {
        // The executable and outer DSO both define choice. Both DSOs
        // define dso_choice. The expected addresses come from llvm-nm.
        let files = fixtures();
        let objects: [Object<'_>; 3] =
            core::array::from_fn(|i| Object::parse(NAMES[i], &files[i].file, BIASES[i]).unwrap());
        let at = |i: usize, symbol: &str| BIASES[i] + files[i].symbols[symbol];
        let lookup = |order: &[usize], symbol: &[u8]| {
            Object::lookup_scope(order.iter().map(|&i| objects[i]), symbol).unwrap()
        };

        assert_eq!(lookup(&[0, 1, 2], b"choice"), Some(at(0, "choice")));
        assert_eq!(lookup(&[0, 1, 2], b"dso_choice"), Some(at(1, "dso_choice")));
        assert_eq!(lookup(&[1, 2], b"choice"), Some(at(1, "choice")));
        assert_eq!(lookup(&[2, 1], b"dso_choice"), Some(at(2, "dso_choice")));
        assert_eq!(lookup(&[1, 2], b"missing_weak"), None);
    }
}

mod aarch64 {
    use super::*;

    // AAELF64 2025Q1 §5.7.15 and the non-TLS set in x2-aarch64.json.
    const KINDS: [(u32, &str); 4] = [
        (257, "R_AARCH64_ABS64"),
        (1025, "R_AARCH64_GLOB_DAT"),
        (1026, "R_AARCH64_JUMP_SLOT"),
        (1027, "R_AARCH64_RELATIVE"),
    ];

    fn fixtures() -> &'static [Fixture; 3] {
        static FIXTURES: OnceLock<[Fixture; 3]> = OnceLock::new();
        FIXTURES.get_or_init(|| build_fixtures("aarch64", FILES))
    }

    #[test]
    fn tls_kinds_resolve_aarch64() {
        let [fixture] = build_fixtures("aarch64", ["libtls.so"]);
        let first = Object::parse(b"/first.so", &fixture.file, BIASES[0]).unwrap();
        let second = Object::parse(b"/second.so", &fixture.file, BIASES[1]).unwrap();
        let template = second.tls_module().unwrap();
        assert_eq!((template.align, template.file_size, template.mem_size), (32, 8, 48));
        let mut patches = BTreeMap::new();
        second.relocate(&[first, second], |Patch { address, value }| {
            assert!(patches.insert(address, value).is_none());
        }).unwrap();

        // AAELF64 §5.7.11: two words per descriptor. Variant I puts the first
        // aligned template at TP+32 and the second at TP+96. The named
        // definition binds to the first object, and the null symbol names
        // the second object's template plus the signed relocation addend.
        let descriptors: Vec<_> = fixture.readelf.lines().filter_map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.get(2) != Some(&"R_AARCH64_TLSDESC") {
                return None;
            }
            let slot = BIASES[1] + hex(fields[0]);
            let pair = [patches[&slot], patches[&(slot + 8)]];
            let expected = if hex(fields[1]) >> 32 == 0 {
                assert_eq!(hex(fields[3]), 8);
                104
            } else {
                assert_eq!(&fields[4..], ["initial", "+", "0"]);
                32
            };
            assert_eq!(pair[1], expected);
            (pair[0] != 0).then_some((pair, expected as usize))
        }).collect();
        assert_eq!(descriptors.len(), 2);
        assert_eq!(patches.len(), 4);

        let offset = |pair: &[u64; 2]| {
            // SAFETY: word zero is the loader's resolver and pair is the
            // two-word descriptor it expects in x0 (the first argument).
            let resolver: unsafe extern "C" fn(*const u64) -> usize =
                unsafe { std::mem::transmute(pair[0] as usize) };
            // SAFETY: the resolver reads only the two live descriptor words.
            unsafe { resolver(pair.as_ptr()) }
        };
        let new_thread_block = || {
            let mut block = [0u64; 18];
            block[4] = 73;
            block[12] = 73;
            block
        };
        let mut main = new_thread_block();
        for (pair, expected) in &descriptors {
            assert_eq!(offset(pair), *expected);
            assert_eq!(main[expected / 8], if *expected == 32 { 73 } else { 0 });
            main[expected / 8] = 91;
        }
        let main_address = main.as_ptr() as usize;
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let mut child = new_thread_block();
                assert_ne!(child.as_ptr() as usize, main_address);
                for (pair, expected) in &descriptors {
                    let slot = offset(pair) / 8;
                    assert_eq!(child[slot], if *expected == 32 { 73 } else { 0 });
                    child[slot] = 19;
                    assert_eq!(child[offset(pair) / 8], 19);
                }
            }).join().unwrap();
        });
        for (pair, _) in &descriptors {
            assert_eq!(main[offset(pair) / 8], 91);
        }
    }

    #[test]
    fn two_dso_fixture_resolves_every_value_aarch64() {
        let files = fixtures();
        let objects: [Object<'_>; 3] =
            std::array::from_fn(|i| Object::parse(NAMES[i], &files[i].file, BIASES[i]).unwrap());
        let mut queue = [objects[0]; 3];
        let mut edges = [Dependency::default(); 3];
        let scope = Scope::load(objects[0], &mut queue, &mut edges, &mut [0; 128], |path| {
            Ok(objects[1..]
                .iter()
                .copied()
                .find(|object| object.name() == path))
        })
        .unwrap();
        assert_eq!(
            scope
                .objects()
                .iter()
                .map(|obj| obj.name())
                .collect::<Vec<_>>(),
            NAMES
        );

        let mut seen = BTreeSet::new();
        for (i, object) in scope.objects().iter().enumerate() {
            let mut expected = BTreeMap::new();
            for line in files[i].readelf.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                let Some(kind) = fields.get(2).filter(|kind| kind.starts_with("R_AARCH64_")) else {
                    continue;
                };
                seen.insert(*kind);
                let address = BIASES[i] + hex(fields[0]);
                let value = if *kind == "R_AARCH64_RELATIVE" {
                    assert_eq!(hex(fields[1]) >> 32, 0);
                    BIASES[i] + hex(fields[3])
                } else {
                    let symbol = fields[4];
                    let definition = files
                        .iter()
                        .zip(BIASES)
                        .find_map(|(file, bias)| file.symbols.get(symbol).map(|value| bias + value))
                        .unwrap_or_else(|| {
                            assert_eq!(symbol, "missing_weak");
                            0
                        });
                    assert_eq!(fields[5], "+");
                    definition + hex(fields[6])
                };
                assert!(expected.insert(address, value).is_none());
            }

            assert!(!expected.is_empty(), "{}", FILES[i]);
            let mut actual = BTreeMap::new();
            object
                .relocate(scope.objects(), |Patch { address, value }| {
                    assert!(actual.insert(address, value).is_none());
                })
                .unwrap();
            assert_eq!(actual, expected, "{}", FILES[i]);
        }
        assert_eq!(seen, KINDS.map(|(_, name)| name).into_iter().collect());
    }

    #[test]
    fn relocate_is_error_not_panic() {
        let fixture = &fixtures()[0];
        let offset = fixture.first_rela();
        let slot = u64::from_le_bytes(fixture.file[offset..offset + 8].try_into().unwrap());
        let truncated = &fixture.file[..fixture.file_offset("DYNAMIC ") + 7];
        assert_eq!(
            Object::parse(NAMES[0], truncated, BIASES[0])
                .unwrap_err()
                .kind,
            ErrorKind::Bounds
        );
        let error = |slot: u64, kind: u32, symbol: u32| {
            let mut corrupt = fixture.file.clone();
            corrupt[offset..offset + 8].copy_from_slice(&slot.to_le_bytes());
            corrupt[offset + 8..offset + 12].copy_from_slice(&kind.to_le_bytes());
            corrupt[offset + 12..offset + 16].copy_from_slice(&symbol.to_le_bytes());
            let obj = Object::parse(NAMES[0], &corrupt, BIASES[0]).unwrap();
            let result = obj
                .relocate(&[obj], |_| panic!("invalid first write"))
                .unwrap_err();
            assert_eq!(result.object, NAMES[0]);
            assert_eq!(result.offset, offset as u64);
            result.kind
        };
        for (kind, name) in KINDS {
            assert_eq!(error(u64::MAX, kind, 0), ErrorKind::Bounds, "{name}");
            assert_eq!(
                error(slot + 1, kind, 0),
                ErrorKind::InvalidRelocation,
                "{name}"
            );
            assert_eq!(
                error(slot, kind, u32::MAX),
                ErrorKind::InvalidRelocation,
                "{name}"
            );
        }
        assert_eq!(error(slot, 1031, 0), ErrorKind::InvalidRelocation);
        for kind in [8, u32::MAX] {
            assert_eq!(error(slot, kind, 0), ErrorKind::UnsupportedRelocation(kind));
        }
    }

    #[test]
    fn ld_breenix_is_self_contained_dyn() {
        let path = std::env::var_os("BREENIX_LD_BREENIX_AARCH64")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../userspace/programs/target/kd/aarch64/ld-breenix.so")
            });
        assert!(
            path.is_file(),
            "build the AArch64 interpreter with --arch aarch64 --kd-loader: {}",
            path.display()
        );
        let inspect = |setting, fallback: &str, args: &[&str]| {
            let tool = std::env::var_os(setting).unwrap_or_else(|| fallback.into());
            let result = Command::new(tool).args(args).arg(&path).output().unwrap();
            assert!(result.status.success(), "{result:?}");
            String::from_utf8(result.stdout).unwrap()
        };
        let elf = inspect(
            "BREENIX_TOOLCHAIN_READELF",
            "llvm-readelf",
            &["-W", "-h", "-l", "-d", "--dyn-relocations"],
        );
        let field = |tag| {
            elf.lines()
                .find_map(|line| line.trim().strip_prefix(tag))
                .unwrap()
                .trim()
        };
        assert_eq!(field("Type:").split_whitespace().next(), Some("DYN"));
        assert_eq!(field("Machine:"), "AArch64");
        let entry = hex(field("Entry point address:"));
        assert!(elf.lines().any(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            fields.first() == Some(&"LOAD")
                && fields[6..fields.len() - 1].contains(&"E")
                && (hex(fields[2])..hex(fields[2]) + hex(fields[4])).contains(&entry)
        }));
        assert!(!elf
            .lines()
            .any(|line| line.trim_start().starts_with("INTERP ")));
        for kind in ["NEEDED", "REL", "RELR", "JMPREL", "TEXTREL"] {
            assert!(
                !elf.contains(&format!("({kind})")),
                "{kind} in the interpreter"
            );
        }
        let symbols = inspect("BREENIX_TOOLCHAIN_NM", "llvm-nm", &["-P", "--defined-only"]);
        let start = symbols
            .lines()
            .find_map(|line| line.strip_prefix("_start "))
            .unwrap();
        assert_eq!(entry, hex(start.split_whitespace().nth(1).unwrap()));
        let undefined = inspect("BREENIX_TOOLCHAIN_NM", "llvm-nm", &["--undefined-only"]);
        assert!(undefined.trim().is_empty(), "{undefined}");

        let mut count = 0;
        for line in elf.lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if let Some(kind) = fields.get(2).filter(|kind| kind.starts_with("R_")) {
                assert_eq!(*kind, "R_AARCH64_RELATIVE");
                assert_eq!(hex(fields[1]), 1027, "bootstrap requires a null symbol");
                count += 1;
            }
        }
        assert!(
            count > 0,
            "the interpreter must have self-relocations to inspect"
        );
    }
}
