//! Hit-test the production row through Slint pointer dispatch, without a desktop.
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, PointerEventButton, WindowAdapter, WindowEvent};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

slint::slint! {
    import { ProcessEntry, Proc } from "../ui/app.slint";
    export component EntryTest inherits Window {
        width: 336px;
        height: 30px;
        in-out property <Proc> item;
        callback toggled(bool);
        ProcessEntry {
            item: root.item;
            toggled(on) => { root.toggled(on); }
        }
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn click(ui: &EntryTest, button: PointerEventButton) {
    let position = slint::LogicalPosition::new(16.0, 15.0);
    ui.window().dispatch_event(WindowEvent::PointerMoved { position });
    ui.window().dispatch_event(WindowEvent::PointerPressed { position, button });
    ui.window().dispatch_event(WindowEvent::PointerReleased { position, button });
}

#[test]
fn process_checkbox_accepts_left_clicks_and_ignores_right_clicks() {
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

    click(&ui, PointerEventButton::Left);
    assert!(ui.get_item().checked, "checkbox must receive the first left click");
    click(&ui, PointerEventButton::Left);
    assert!(!ui.get_item().checked, "second click must uncheck");
    click(&ui, PointerEventButton::Left);
    assert_eq!(*events.borrow(), vec![true, false, true]);

    // Replacing model data and reopening the panel must not break the binding.
    ui.set_item(ui.get_item());
    ui.hide().unwrap();
    ui.show().unwrap();
    click(&ui, PointerEventButton::Left);
    assert!(!ui.get_item().checked);
    assert_eq!(*events.borrow(), vec![true, false, true, false]);

    click(&ui, PointerEventButton::Right);
    assert!(!ui.get_item().checked, "opening the whitelist menu must not select a process");
    assert_eq!(events.borrow().len(), 4, "right click must not emit toggled");
    ui.hide().unwrap();
}
