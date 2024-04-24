use gpui::{div, IntoElement, ParentElement, Render, Styled, View, VisualContext, WindowContext};
use indoc::indoc;
use story::{StoryContainer, StoryItem, StorySection};

pub struct UiTextFieldStory;

impl UiTextFieldStory {
    pub fn view(cx: &mut WindowContext) -> View<Self> {
        cx.new_view(|_cx| Self)
    }
}

impl Render for UiTextFieldStory {
    fn render(&mut self, _cx: &mut gpui::ViewContext<Self>) -> impl IntoElement {
        StoryContainer::new(
            "UiTextField Story",
            "crates/storybook/src/stories/ui_text_field.rs",
        )
        .children(
        vec![
        StorySection::new()
        .child(
            StoryItem::new("Default",
                div().bg(gpui::blue()).child("Hello World!")).usage(
                indoc! {r##"
                            div()
                                .child("Hello World!")
                            "##
                },
            ),
        )
        .child(
            StoryItem::new("tbd",
            div().flex().w_96().bg(gpui::blue()).child(div().overflow_hidden().child(concat!(
                    "flex-row. width 96. overflow-hidden. The quick brown fox jumps over the lazy dog. ",
                    "Meanwhile, the lazy dog decided it was time for a change. ",
                    "He started daily workout routines, ate healthier and became the fastest dog in town.",
                )))
            )
        )
        ]
        )
        .into_element()
    }
}
