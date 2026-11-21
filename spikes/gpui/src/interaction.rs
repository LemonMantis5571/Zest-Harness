//! One reply channel belongs to one prompt in one chat. Dropping a card denies it.
use crate::bridge::{Ev, EvKind};
use futures::channel::mpsc::UnboundedSender;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::oneshot;
use zest_core::*;

pub enum Request {
    Approval {
        title: String,
        preview: String,
    },
    Question {
        prompt: String,
        choices: Vec<String>,
        multiple: bool,
    },
}

pub enum Response {
    Allow,
    Deny,
    Answer(Vec<String>),
}

pub struct Card {
    pub request: Request,
    pub reply: oneshot::Sender<Response>,
    pub selected: Vec<String>,
}

pub struct Interactions {
    pub events: UnboundedSender<Ev>,
    pub policy: Arc<Mutex<ApprovalPolicy>>,
}

impl Interactions {
    async fn ask(&self, request: Request) -> Response {
        let (reply, receiver) = oneshot::channel();
        let card = Card {
            request,
            reply,
            selected: Vec::new(),
        };
        if self
            .events
            .unbounded_send(Ev {
                at: Instant::now(),
                kind: EvKind::Card(card),
            })
            .is_err()
        {
            return Response::Deny;
        }
        receiver.await.unwrap_or(Response::Deny)
    }

    async fn approve(&self, title: String, preview: String) -> bool {
        matches!(
            self.ask(Request::Approval { title, preview }).await,
            Response::Allow
        )
    }

    async fn question(
        &self,
        prompt: String,
        choices: Vec<String>,
        multiple: bool,
    ) -> Option<Vec<String>> {
        match self
            .ask(Request::Question {
                prompt,
                choices,
                multiple,
            })
            .await
        {
            Response::Answer(answers) if !answers.is_empty() => Some(answers),
            _ => None,
        }
    }
}

#[async_trait::async_trait]
impl Approver for Interactions {
    async fn decide(&self, req: &ApprovalRequest) -> ApprovalDecision {
        let preview = format!(
            "{}\n{}\n{}",
            req.preview.path, req.preview.summary, req.preview.diff
        );
        if self
            .approve(
                format!("Allow {}? ({:?})", req.tool_name, req.risk),
                preview,
            )
            .await
        {
            ApprovalDecision::AllowOnce
        } else {
            ApprovalDecision::Deny
        }
    }
}

#[async_trait::async_trait]
impl Questioner for Interactions {
    async fn answer(&self, req: &QuestionRequest) -> std::result::Result<String, String> {
        self.question(req.prompt.clone(), req.choices.clone(), req.multiple)
            .await
            .map(|answers| answers.join("\n"))
            .ok_or_else(|| "Question dismissed".into())
    }
}

#[async_trait::async_trait]
impl ProviderInteractionHost for Interactions {
    fn approval_policy(&self) -> Option<Arc<Mutex<ApprovalPolicy>>> {
        Some(self.policy.clone())
    }

    async fn approve_command(&self, req: ProviderCommandRequest) -> bool {
        self.approve(
            "Allow command?".into(),
            format!(
                "{}\n{}\n{}",
                req.cwd.unwrap_or_default(),
                req.command,
                req.reason.unwrap_or_default()
            ),
        )
        .await
    }

    async fn approve_file_change(&self, req: ProviderFileChangeRequest) -> bool {
        self.approve(
            "Allow file change?".into(),
            format!(
                "{}\n{}\n{}",
                req.path.unwrap_or_default(),
                req.reason.unwrap_or_default(),
                req.diff.unwrap_or_default()
            ),
        )
        .await
    }

    async fn answer_question(&self, req: ProviderQuestionRequest) -> Option<Vec<String>> {
        self.question(req.prompt, req.choices, req.multiple).await
    }
}
