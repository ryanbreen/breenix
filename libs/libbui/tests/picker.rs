//! File-picker interactions from §15, including the inclusive time/distance edges.
use libbui::widget::file_picker::{FileEntry, FilePicker, FilePickerResult};
use libbui::{InputState, Key, KeyInput, Rect, Theme};
fn picker(count: usize) -> FilePicker {
    let mut entries = vec![FileEntry {
        name: b"Dir".to_vec(),
        is_dir: true,
        size: 0,
    }];
    for i in (0..count).rev() {
        entries.push(FileEntry {
            name: format!("file{i:03}").into_bytes(),
            is_dir: false,
            size: 10,
        });
    }
    FilePicker::new(
        Rect::new(0, 0, 300, 240),
        b"/data".to_vec(),
        entries,
        &Theme::dark(),
    )
}
fn press(x: i32, y: i32, t: u64) -> InputState {
    InputState::from_raw(x, y, 1, 0, t)
}
#[test]
fn sort_keys_and_timestamped_double_click() {
    let mut p = picker(5);
    assert_eq!(p.update(&press(50, 30, 1000)), FilePickerResult::Active);
    assert_eq!(p.selected_entry().unwrap().name, b"Dir");
    assert_eq!(p.update(&press(50, 30, 1401)), FilePickerResult::Active);
    assert_eq!(p.update(&press(55, 30, 1801)), FilePickerResult::Active);
    assert_eq!(
        p.update(&press(59, 34, 2201)),
        FilePickerResult::NavigateDir(0)
    );
    p.key(&KeyInput::new(Key::Down));
    assert_eq!(p.selected_entry().unwrap().name, b"file000");
    assert_eq!(
        p.key(&KeyInput::new(Key::Enter)),
        FilePickerResult::Selected(1)
    );
}
#[test]
fn path_submission_scroll_and_error_remove_stale_entries() {
    let mut p = picker(60);
    p.key(&KeyInput::new(Key::End));
    assert_eq!(p.selected_entry().unwrap().name, b"file059");
    assert!(p.scroll_offset() > 0);
    p.key(&KeyInput::new(Key::Home));
    assert_eq!(p.scroll_offset(), 0);
    p.scroll(-1);
    assert_eq!(p.scroll_offset(), 3);
    p.scroll(1);
    assert_eq!(p.scroll_offset(), 0);
    p.key(&KeyInput::new(Key::Tab));
    p.key(&KeyInput::new(Key::Home));
    p.key(&KeyInput {
        shift: true,
        ..KeyInput::new(Key::End)
    });
    p.key(&KeyInput::text("/other/é"));
    assert_eq!(
        p.key(&KeyInput::new(Key::Enter)),
        FilePickerResult::NavigatePath("/other/é".as_bytes().to_vec())
    );
    p.update(&press(250, 200, 5000));
    p.show_error(b"Permission denied");
    assert!(p.selected_entry().is_none());
    assert_eq!(p.error(), Some(&b"Permission denied"[..]));
    assert_eq!(p.key(&KeyInput::new(Key::Enter)), FilePickerResult::Active);
}

#[test]
fn clicking_more_scrolls_instead_of_selecting_the_covered_row() {
    let mut p = picker(60);
    assert_eq!(p.update(&press(150, 197, 1000)), FilePickerResult::Active);
    assert!(p.scroll_offset() > 0);
    assert!(p.selected_entry().is_none());
    assert_eq!(p.update(&press(150, 27, 1500)), FilePickerResult::Active);
    assert_eq!(p.scroll_offset(), 0);
    assert!(p.selected_entry().is_none());
}
