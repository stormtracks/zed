use gpui::*;

struct HelloWorld {
    text: SharedString,
}

actions!(hw, [Quit, QuitLocal]);

pub fn quitme() {
    println!("Local Quitting...");
}

impl Render for HelloWorld {
    fn render(&mut self, _cx: &mut ViewContext<Self>) -> impl IntoElement {
        div()
            .key_context("parent")
            .flex()
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
    App::new().run(|cx: &mut AppContext| {
        cx.activate(true);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.on_action(|_: &QuitLocal, _cx| quitme());

        cx.bind_keys([
            KeyBinding::new("cmd-c", QuitLocal, Some("parent")),
            KeyBinding::new("cmd-q", Quit, Some("parent")),
        ]);

        cx.set_menus(vec![Menu {
            name: "HelloWorld",
            items: vec![
                MenuItem::action("QuitLocal", QuitLocal),
                MenuItem::action("Quit", Quit),
            ],
        }]);

        let bounds = Bounds::centered(None, size(px(600.0), px(600.0)), cx);
        cx.open_window(
            WindowOptions {
                bounds: Some(bounds),
                ..Default::default()
            },
            |cx| {
                cx.new_view(|_cx| HelloWorld {
                    text: "World".into(),
                })
            },
        );
    });
}
