use smbfs::shares::parse_shares;

#[test]
fn bad_lines_are_reported_and_good_lines_load() {
    let table = "# shares\n\n//server/one /mnt/one smb credfile=/etc/one\n\
                 //server/missing /mnt/missing smb\n\
                 //server/type /mnt/type nfs credfile=/etc/type\n\
                 //server/relative mnt/relative smb credfile=/etc/relative\n\
                 //server/options /mnt/options smb port=0\n\
                 # another comment\n\
                 //server/two /mnt/two smb credfile=/etc/two,port=1445\n";
    let (entries, errors) = parse_shares(table);
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].source, "//server/one");
    assert_eq!(entries[0].directory, "/mnt/one");
    assert_eq!(entries[0].options, "credfile=/etc/one");
    assert_eq!(entries[1].source, "//server/two");
    assert_eq!(entries[1].directory, "/mnt/two");
    assert_eq!(entries[1].options, "credfile=/etc/two,port=1445");
    assert_eq!(errors.iter().map(|error| error.line).collect::<Vec<_>>(), [4, 5, 6, 7]);
    assert!(errors.iter().all(|error| !error.message.is_empty()));
}
