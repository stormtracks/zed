use gpui::{
    div, green, red, HighlightStyle, InteractiveText, IntoElement, ParentElement, Render, Styled,
    StyledText, View, VisualContext, WindowContext,
};
use indoc::indoc;
use story::*;

pub struct UiTextFieldStory;

impl UiTextFieldStory {
    pub fn view(cx: &mut WindowContext) -> View<Self> {
        cx.new_view(|_cx| Self)
    }
}

impl Render for UiTextFieldStory {
    fn render(&mut self, cx: &mut gpui::ViewContext<Self>) -> impl IntoElement {
        StoryContainer::new(
            "UiTextField Story",
            "crates/storybook/src/stories/ui_text_field.rs",
        )
        .children(vec![StorySection::new().child(
            StoryItem::new("Default", div().bg(gpui::blue()).child("Hello World!")).usage(
                indoc! {r##"
                            div()
                                .child("Hello World!")
                            "##
                },
            ),
        )])
        .into_element()
    }
}
