//! Capacity for the MMIO block transports, including root, fixture and scratch disks.

use syn::{Expr, Item, Type};

const DRIVER: &str = include_str!("../kernel/src/drivers/virtio/block_mmio.rs");
const TRANSPORT: &str = include_str!("../kernel/src/drivers/virtio/mmio.rs");

fn named(expr: &Expr, name: &str) -> bool {
    matches!(expr, Expr::Path(path) if path.path.is_ident(name))
}

fn constant(source: &str, name: &str) -> Box<Expr> {
    syn::parse_file(source)
        .unwrap()
        .items
        .into_iter()
        .find_map(|item| match item {
            Item::Const(item) if item.ident == name => Some(item.expr),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing {name}"))
}

fn integer(expr: &Expr) -> usize {
    let Expr::Lit(value) = expr else {
        panic!("expected an integer constant");
    };
    let syn::Lit::Int(value) = &value.lit else {
        panic!("expected an integer literal");
    };
    value.base10_parse().unwrap()
}

#[test]
fn block_limit_covers_the_mmio_transports_and_three_disks() {
    let transports = integer(&constant(TRANSPORT, "VIRTIO_MMIO_COUNT"));
    let limit = constant(DRIVER, "MAX_BLOCK_DEVICES");
    let capacity = if named(&limit, "VIRTIO_MMIO_COUNT") {
        transports
    } else {
        integer(&limit)
    };
    assert!(
        capacity >= 3,
        "root, read fixture and scratch need three disks"
    );
    assert_eq!(
        capacity, transports,
        "block capacity must cover the probe range"
    );
}

#[test]
fn per_device_tables_use_the_block_limit() {
    let source = syn::parse_file(DRIVER).unwrap();
    for name in [
        "REQUEST_GATES",
        "COMPLETIONS",
        "BLOCK_DEVICES",
        "QUEUE_MEM",
        "REQ_HEADERS",
        "DATA_BUFS",
        "STATUS_BUFS",
    ] {
        let table = source
            .items
            .iter()
            .find_map(|item| match item {
                Item::Static(item) if item.ident == name => Some(item),
                _ => None,
            })
            .unwrap_or_else(|| panic!("missing per-device table {name}"));
        let Type::Array(array) = &*table.ty else {
            panic!("{name} must be a per-device array");
        };
        assert!(
            named(&array.len, "MAX_BLOCK_DEVICES"),
            "{name} array length"
        );
        let Expr::Repeat(initializer) = &*table.expr else {
            panic!("{name} must initialize MAX_BLOCK_DEVICES entries");
        };
        assert!(
            named(&initializer.len, "MAX_BLOCK_DEVICES"),
            "{name} initializer length"
        );
    }
}
