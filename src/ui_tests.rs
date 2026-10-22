//! Hit-test the production row through Slint pointer dispatch, without a desktop.
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { ProcessEntry, Proc, AppTile, App } from "../ui/app.slint";
    export component EntryTest inherits Window {
        width: 336px;
        height: root.tile ? 100px : 30px;
        in-out property <bool> tile;
        callback launched();
        in-out property <App> app;
        in-out property <Proc> item;
        in-out property <bool> editable: true;
        callback toggled(bool);
        if !root.tile: ProcessEntry {
            item: root.item;
            editable: root.editable;
            toggled(on) => { root.toggled(on); }
        }
        if root.tile: AppTile {
            x: 0px; y: 0px; width: 84px; height: 92px;
            item: root.app;
            launch => { root.launched(); }
        }
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn click_at(ui: &EntryTest, button: PointerEventButton, x: f32, y: f32) {
    let position = slint::LogicalPosition::new(x, y);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
}

#[test]
fn process_rows_and_shortcut_tiles_receive_mouse_input() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::NewBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let ui = EntryTest::new().unwrap();
    ui.set_item(Proc { label: "Visual Studio Code".into(), ..Default::default() });
    let events = Rc::new(RefCell::new(Vec::new()));
    let events2 = events.clone();
    let weak = ui.as_weak();
    ui.on_toggled(move |on| {
        events2.borrow_mut().push(on);
        let ui = weak.upgrade().unwrap();
        let mut row = ui.get_item();
        row.checked = on;
        ui.set_item(row);
    });
    ui.show().unwrap();
    window.set_size(slint::PhysicalSize::new(336, 30));
    let mut pixels = vec![slint::Rgb8Pixel::default(); 336 * 30];
    window.draw_if_needed(|renderer| { renderer.render(&mut pixels, 336); });

    click_at(&ui, PointerEventButton::Left, 16.0, 15.0);
    assert!(ui.get_item().checked, "checkbox must receive the first left click");
    click_at(&ui, PointerEventButton::Left, 120.0, 15.0);
    assert!(!ui.get_item().checked, "second click must uncheck");
    click_at(&ui, PointerEventButton::Left, 44.0, 15.0);
    assert_eq!(*events.borrow(), vec![true, false, true]);

    // Replacing model data and reopening the panel must not break the binding.
    ui.set_item(ui.get_item());
    ui.hide().unwrap();
    ui.show().unwrap();
    click_at(&ui, PointerEventButton::Left, 330.0, 15.0);
    assert!(!ui.get_item().checked);
    assert_eq!(*events.borrow(), vec![true, false, true, false]);

    ui.set_editable(false);
    click_at(&ui, PointerEventButton::Left, 16.0, 15.0);
    click_at(&ui, PointerEventButton::Left, 120.0, 15.0);
    assert!(!ui.get_item().checked);
    assert_eq!(events.borrow().len(), 4, "selection must stay locked during boost");
    ui.set_editable(true);

    click_at(&ui, PointerEventButton::Right, 120.0, 15.0);
    assert!(!ui.get_item().checked, "opening the whitelist menu must not select a process");
    assert_eq!(events.borrow().len(), 4, "right click must not emit toggled");
    ui.window().dispatch_event(WindowEvent::KeyPressed { text: slint::platform::Key::Escape.into() });
    ui.set_app(App { name: "Example".into(), ..Default::default() });
    ui.set_tile(true);
    window.set_size(slint::PhysicalSize::new(336, 100));
    let launches = Rc::new(RefCell::new(0));
    let count = launches.clone();
    ui.on_launched(move || *count.borrow_mut() += 1);
    let mut pixels = vec![slint::Rgb8Pixel::default(); 336 * 100];
    window.draw_if_needed(|renderer| { renderer.render(&mut pixels, 336); });
    click_at(&ui, PointerEventButton::Left, 42.0, 30.0);
    assert_eq!(*launches.borrow(), 1, "clicking the shortcut icon must launch once");
    click_at(&ui, PointerEventButton::Left, 42.0, 75.0);
    assert_eq!(*launches.borrow(), 2, "clicking shortcut text must launch once");
    ui.hide().unwrap();
}
