use gpui::{Menu, MenuItem};

pub fn app_menus() -> Vec<Menu> {
    use zed_actions::Quit;
    vec![Menu {
        name: "Zed".into(),
        items: vec![
            MenuItem::action("About Zed…", zed_actions::About),
            MenuItem::action("Quit", Quit),
        ],
    }]
}
