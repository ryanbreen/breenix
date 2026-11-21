//! Exercise the firmware's allocation planner with a populated BreenFS image.
#[path = "../scripts/metal/efi-stub/src/breenfs_allocation.rs"]
mod allocation;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut source = File::open(args.next().unwrap()).unwrap();
    let size = source.metadata().unwrap().len();
    let total = size as usize / allocation::BLOCK;
    let expected: Vec<_> = std::fs::read_to_string(args.next().unwrap())
        .unwrap()
        .lines()
        .flat_map(|line| {
            let (first, count) = line.split_once(' ').unwrap();
            let first = first.parse::<usize>().unwrap();
            first..first + count.parse::<usize>().unwrap()
        })
        .collect();
    let mut bitmap = vec![0; total.div_ceil(8)];
    let mut visited = bitmap.clone();
    let mut scratch = vec![0; allocation::SCRATCH];
    let mut selected = File::create(args.next().unwrap()).unwrap();
    selected.set_len(size).unwrap();
    let count = allocation::discover(
        total,
        &mut bitmap,
        &mut visited,
        &mut scratch,
        |block, bytes| {
            source.seek(SeekFrom::Start((block * allocation::BLOCK) as u64)).unwrap();
            source.read_exact(bytes).unwrap();
            Ok(())
        },
    ).unwrap();
    let blocks: Vec<_> = (0..total)
        .filter(|block| bitmap[block / 8] & (1 << (block % 8)) != 0)
        .collect();
    assert_eq!(count, blocks.len());
    assert_eq!(blocks, expected);
    let mut bytes = [0; allocation::BLOCK];
    use std::io::Write;
    for block in blocks {
        let offset = (block * allocation::BLOCK) as u64;
        source.seek(SeekFrom::Start(offset)).unwrap();
        source.read_exact(&mut bytes).unwrap();
        selected.seek(SeekFrom::Start(offset)).unwrap();
        selected.write_all(&bytes).unwrap();
    }
}
