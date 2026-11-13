use crate::ext2::{self, dir::DirReader, Ext2Fs};

fn links(fs: &Ext2Fs, ino: u32) -> u16 {
    fs.read_inode(ino).unwrap().i_links_count
}

fn entries(fs: &Ext2Fs, ino: u32) -> Vec<(Vec<u8>, u32)> {
    let inode = fs.read_inode(ino).unwrap();
    let data = fs.read_directory(&inode).unwrap();
    DirReader::new(&data)
        .map(|entry| (entry.name.into_bytes(), entry.inode))
        .collect()
}

#[test]
fn link_rename_unlink_rmdir_on_fixture() {
    let (mut fs, _, _) = ext2::reclaim_checks::directory_fixture();
    let root = ext2::EXT2_ROOT_INO;
    let dir = fs.create_directory_in(root, "work", 0o755, 0, 0).unwrap();
    assert_eq!(links(&fs, root), 3);
    assert_eq!(links(&fs, dir), 2);
    assert_eq!(
        fs.lookup_in_dir(&fs.read_inode(root).unwrap(), "work")
            .unwrap(),
        Some(dir)
    );
    assert_eq!(
        fs.lookup_in_dir(&fs.read_inode(dir).unwrap(), "..")
            .unwrap(),
        Some(root)
    );

    let file = fs.create_node(dir, "original", 0o100644, 0, 0, 0).unwrap();
    assert_eq!(links(&fs, file), 1);
    fs.link_in_dir(file, root, "alias").unwrap();
    assert_eq!(links(&fs, file), 2);
    assert!(entries(&fs, root).contains(&(b"alias".to_vec(), file)));

    let root_handle = fs.pin_inode(root).unwrap();
    let dir_handle = fs.pin_inode(dir).unwrap();
    fs.rename(&root_handle, "alias", &dir_handle, "renamed", false)
        .unwrap();
    drop(dir_handle);
    drop(root_handle);
    assert_eq!(links(&fs, file), 2);
    assert_eq!(
        fs.lookup_in_dir(&fs.read_inode(root).unwrap(), "alias")
            .unwrap(),
        None
    );
    let mut names = entries(&fs, dir);
    names.sort();
    assert_eq!(
        names,
        vec![
            (b".".to_vec(), dir),
            (b"..".to_vec(), root),
            (b"original".to_vec(), file),
            (b"renamed".to_vec(), file),
        ]
    );
    assert_eq!(
        fs.remove_directory_in(root, "work"),
        Err("Directory not empty")
    );
    assert_eq!(links(&fs, root), 3);

    fs.create_symlink_in("work/renamed", root, "symbolic", 0, 0)
        .unwrap();
    let symlink = fs
        .lookup_in_dir(&fs.read_inode(root).unwrap(), "symbolic")
        .unwrap()
        .unwrap();
    assert_eq!(fs.read_symlink_bytes(symlink).unwrap(), b"work/renamed");
    assert_eq!(links(&fs, symlink), 1);
    fs.unlink_in_dir(root, "symbolic").unwrap();
    assert_eq!(links(&fs, symlink), 0);

    fs.unlink_in_dir(dir, "original").unwrap();
    assert_eq!(links(&fs, file), 1);
    assert_eq!(
        fs.lookup_in_dir(&fs.read_inode(dir).unwrap(), "original")
            .unwrap(),
        None
    );
    assert_eq!(
        fs.lookup_in_dir(&fs.read_inode(dir).unwrap(), "renamed")
            .unwrap(),
        Some(file)
    );
    fs.unlink_in_dir(dir, "renamed").unwrap();
    assert_eq!(links(&fs, file), 0);
    assert_eq!(
        entries(&fs, dir),
        vec![(b".".to_vec(), dir), (b"..".to_vec(), root)]
    );
    fs.remove_directory_in(root, "work").unwrap();
    assert_eq!(links(&fs, root), 2);
    assert_eq!(links(&fs, dir), 0);
    assert!(entries(&fs, root).is_empty());
}
