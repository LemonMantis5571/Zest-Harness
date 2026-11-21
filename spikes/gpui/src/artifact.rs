//! Inert chat cards for explicitly published interactive documents.
use crate::{Root, native, native_view::button, web_content};
use gpui::{prelude::*, *};
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag};
use std::{path::Path, sync::Arc};
use zest_core::{
    html::{HTML_CSP, HtmlDocument},
    tools::ToolMetadata,
};

pub fn metadata_document(metadata: Option<&ToolMetadata>) -> Option<Arc<HtmlDocument>> {
    let ToolMetadata::HtmlDocument { title, html } = metadata? else {
        return None;
    };
    HtmlDocument::parse(title.clone(), html.clone())
        .ok()
        .map(Arc::new)
}

pub fn fenced_documents(text: &str) -> Vec<Arc<HtmlDocument>> {
    Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            let Event::Start(Tag::CodeBlock(CodeBlockKind::Fenced(info))) = event else {
                return None;
            };
            if info.split_whitespace().next()? != "zest-html" {
                return None;
            }
            let (opening, rest) = text.get(range)?.split_once('\n')?;
            let opening = opening.trim_start_matches(' ');
            let marker = opening.chars().next()?;
            let count = opening.chars().take_while(|ch| *ch == marker).count();
            let title = opening[count..].strip_prefix("zest-html")?.trim();
            if title.contains('`') {
                return None;
            }
            let (body, closing) = rest.trim_end_matches(['\r', '\n']).rsplit_once('\n')?;
            let closing = closing.trim_matches([' ', '\t', '\r']);
            if closing.len() < count || !closing.chars().all(|ch| ch == marker) {
                return None;
            }
            let source = body.strip_suffix('\r').unwrap_or(body);
            HtmlDocument::parse(
                if title.is_empty() {
                    "HTML preview"
                } else {
                    title
                }
                .into(),
                source.into(),
            )
            .ok()
            .map(Arc::new)
        })
        .collect()
}

pub fn card(
    row: usize,
    index: usize,
    document: Arc<HtmlDocument>,
    source_visible: bool,
    cx: &mut Context<Root>,
) -> impl IntoElement + use<> {
    let open = document.clone();
    let save = document.clone();
    let id = |action: &str| SharedString::from(format!("html-{row}-{index}-{action}"));
    let controls = div().flex().items_center().gap_2().child(document.title().to_owned())
        .when(cfg!(windows),|controls|controls.child(button(id("open"),"Open",cx.listener(move |_,_,_,cx| {
            let doc = open.clone();
            let task = native::executor().spawn(async move {web_content::show_artifact(&doc).await});
            cx.spawn(async move |this,cx| {
                let result = task.await.map_err(|e|e.to_string()).and_then(|r|r);
                if let Err(e)=result {let _=this.update(cx,|root,cx|{root.note=e;cx.notify();});}
            }).detach();
        }))))
        .child(button(id("source"),if source_visible {"Hide source"}else{"Source"},cx.listener(move |root,_,_,cx| {
            let previous = root.artifact_source.take();
            if previous != Some((row,index)) { root.artifact_source=Some((row,index)); }
            if let Some((previous_row,_))=previous { root.list.remeasure_items(previous_row..previous_row+1); }
            root.list.remeasure_items(row..row+1);
            cx.notify();
        })))
        .child(button(id("save"),"Save HTML",cx.listener(move |root,_,_,cx| {
            let directory = root.native.as_ref().map(|chat|chat.project.path.as_path()).unwrap_or(Path::new("."));
            let picker = cx.prompt_for_new_path(directory,Some("document.html"));
            let doc = save.clone();
            cx.spawn(async move |this,cx| {
                let result: Result<(),String> = async {
                    let Some(mut path)=picker.await.map_err(|e|e.to_string())?.map_err(|e|e.to_string())? else {return Ok(())};
                    path.set_extension("html");
                    cx.background_executor().spawn(async move {
                        let html=format!("<!doctype html><meta http-equiv=\"Content-Security-Policy\" content=\"{}\">{}",HTML_CSP,doc.isolated_html());
                        zest_core::atomic_write(&path,html.as_bytes()).map_err(|e|e.to_string())
                    }).await
                }.await;
                if let Err(e)=result {let _=this.update(cx,|root,cx|{root.note=e;cx.notify();});}
            }).detach();
        })));
    div()
        .my_1()
        .p_3()
        .flex()
        .flex_col()
        .gap_2()
        .border_1()
        .rounded(px(12.))
        .border_color(rgb(0x25262b))
        .bg(rgb(0x121314))
        .child(controls)
        .when(source_visible, |d| {
            d.child(
                div()
                    .id(id("code"))
                    .max_h(px(240.))
                    .overflow_y_scroll()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(rgb(crate::theme::BORDER))
                    .bg(rgb(0x17181b))
                    .p(px(10.))
                    .font_family(crate::theme::MONO)
                    .text_size(px(12.))
                    .child(document.source().to_owned()),
            )
        })
}

#[cfg(test)]
mod tests {
    use super::fenced_documents;
    #[test]
    fn closed_artifact_fences_preserve_source_and_title() {
        let source = "Example\r\n\r\n~~~zest-html Counter\r\n<h1>first</h1>\r\n<button>second</button>\r\n~~~\r\n\r\nDone.";
        let docs = fenced_documents(source);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title(), "Counter");
        assert_eq!(
            docs[0].source(),
            "<h1>first</h1>\r\n<button>second</button>"
        );
    }
    #[test]
    fn open_plain_and_nested_example_fences_are_inert() {
        assert!(fenced_documents("~~~zest-html Example\n<p>incomplete</p>").is_empty());
        assert!(fenced_documents("~~~html\n<p>example</p>\n~~~").is_empty());
        assert!(
            fenced_documents("~~~~example\n~~~zest-html Nested\n<p>x</p>\n~~~\n~~~~").is_empty()
        );
    }
}
