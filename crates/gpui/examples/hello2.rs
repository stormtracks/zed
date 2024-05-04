use gpui::*;

struct HelloWorld {
    focus: FocusHandle,
    text: SharedString,
}

actions!(ford, [Quit]);

impl HelloWorld {
    pub fn new(cx: &mut ViewContext<HelloWorld>) -> HelloWorld {
        let focus = cx.focus_handle();
        let text = SharedString::from("World");
        HelloWorld { focus, text }
    }
}

impl Render for HelloWorld {
    fn render(&mut self, _cx: &mut ViewContext<Self>) -> impl IntoElement {
        div()
            .id("hello")
            .active(|style| style.bg(red()))
            .track_focus(&self.focus)
            // if key_context is not here it doesn't quit
            .key_context("hello")
            .bg(rgb(0x2e7d32))
            .size(Length::Definite(Pixels(300.0).into()))
            .justify_center()
            .items_center()
            .shadow_lg()
            .border()
            .border_color(rgb(0x0000ff))
            .text_xl()
            .text_color(rgb(0xffffff))
            .child(format!("Hello, {}!", &self.text))
    }
}

fn main() {
    App::new().run(|cx| {
        cx.activate(true);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);

        let bounds = Bounds::centered(None, size(px(600.0), px(600.0)), cx);
        let window_options = WindowOptions {
            bounds: Some(bounds),
            ..Default::default()
        };

        cx.open_window(window_options, |cx| cx.new_view(HelloWorld::new));
    });
}
