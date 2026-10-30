//! A `build` closure that panics inside `Window::open_parented` must not leave a
//! live NSView behind.
//!
//! Plug-in hosts embed the editor with `open_parented` and plug-in frameworks
//! catch a panicking editor so the host survives. Before this test existed, the
//! NSView created for the editor stayed in the host's view hierarchy and in
//! NSNotificationCenter with a null `baseview_state` ivar. The next
//! NSWindowDidBecomeKey/DidResignKey notification then dereferenced that null
//! state in `handle_notification` and crashed the host with SIGSEGV
//! (iZotope RX 10 on macOS, 2026-10-25).
//!
//! AppKit needs the main thread, hence `harness = false`.

#[cfg(target_os = "macos")]
fn main() {
    use baseview::{
        Event, EventStatus, Window, WindowHandler, WindowOpenOptions, WindowScalePolicy,
    };
    use cocoa::appkit::{NSApp, NSApplication, NSBackingStoreType, NSWindow, NSWindowStyleMask};
    use cocoa::base::{id, nil, NO};
    use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
    use objc::{class, msg_send, sel, sel_impl};
    use raw_window_handle::{AppKitWindowHandle, HasRawWindowHandle, RawWindowHandle};
    use std::panic::{catch_unwind, AssertUnwindSafe};

    struct Parent {
        ns_window: id,
        ns_view: id,
    }

    unsafe impl HasRawWindowHandle for Parent {
        fn raw_window_handle(&self) -> RawWindowHandle {
            let mut handle = AppKitWindowHandle::empty();
            handle.ns_window = self.ns_window as *mut _;
            handle.ns_view = self.ns_view as *mut _;
            RawWindowHandle::AppKit(handle)
        }
    }

    struct Never;

    impl WindowHandler for Never {
        fn on_frame(&mut self, _: &mut Window) {}
        fn on_event(&mut self, _: &mut Window, _: Event) -> EventStatus {
            EventStatus::Ignored
        }
    }

    // Every baseview NSView gets its own Objective-C class, disposed in the
    // view's `dealloc`, so a leftover class means a leaked view.
    unsafe fn baseview_view_classes() -> usize {
        extern "C" {
            fn objc_getClassList(buffer: *mut *const objc::runtime::Class, count: i32) -> i32;
        }
        let total = objc_getClassList(std::ptr::null_mut(), 0);
        let mut classes = vec![std::ptr::null(); total as usize];
        let total = objc_getClassList(classes.as_mut_ptr(), total).min(total) as usize;
        classes[..total]
            .iter()
            .filter(|class| (***class).name().starts_with("BaseviewNSView_"))
            .count()
    }

    unsafe fn subview_count(view: id) -> usize {
        let subviews: id = msg_send![view, subviews];
        let count: usize = msg_send![subviews, count];
        count
    }

    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let app = NSApp();
        app.setActivationPolicy_(cocoa::appkit::NSApplicationActivationPolicyAccessory);

        let ns_window = NSWindow::alloc(nil).initWithContentRect_styleMask_backing_defer_(
            NSRect::new(NSPoint::new(0., 0.), NSSize::new(320., 240.)),
            NSWindowStyleMask::NSTitledWindowMask,
            NSBackingStoreType::NSBackingStoreBuffered,
            NO,
        );
        let content: id = ns_window.contentView();
        let parent = Parent { ns_window, ns_view: content };
        assert_eq!(subview_count(content), 0);
        let classes_before = baseview_view_classes();

        let options = WindowOpenOptions {
            title: "failed build".into(),
            size: baseview::Size::new(200.0, 100.0),
            scale: WindowScalePolicy::ScaleFactor(1.0),
            #[cfg(feature = "opengl")]
            gl_config: None,
        };

        // Keep the expected panic message out of the test output.
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let opened = catch_unwind(AssertUnwindSafe(|| {
            Window::open_parented(&parent, options, |_window| -> Never {
                panic!("editor could not be created")
            })
        }));
        std::panic::set_hook(hook);
        assert!(opened.is_err(), "the build closure was expected to panic");

        let left = subview_count(content);
        let leaked = baseview_view_classes() - classes_before;
        println!(
            "after the failed build: {left} subviews in the parent, {leaked} baseview views alive"
        );

        // What a host does next: its window gains and loses key status. Each of
        // these used to reach the leftover view's null state.
        let center: id = msg_send![class!(NSNotificationCenter), defaultCenter];
        for name in ["NSWindowDidBecomeKeyNotification", "NSWindowDidResignKeyNotification"] {
            let name = NSString::alloc(nil).init_str(name);
            let () = msg_send![center, postNotificationName: name object: ns_window];
        }
        println!("key-window notifications delivered without a crash");

        assert_eq!(left, 0, "the failed build left its NSView in the parent");
        assert_eq!(leaked, 0, "the failed build leaked its NSView (still observing notifications)");
        println!("ok");
    }
}

#[cfg(not(target_os = "macos"))]
fn main() {}
