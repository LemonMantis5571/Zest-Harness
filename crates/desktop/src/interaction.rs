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
        allow_session: bool,
    },
    Question {
        prompt: String,
        choices: Vec<String>,
        multiple: bool,
    },
}

pub enum Response {
    Allow,
    AllowSession,
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

    async fn decide_approval(
        &self,
        title: String,
        preview: String,
        allow_session: bool,
    ) -> ApprovalDecision {
        match self
            .ask(Request::Approval {
                title,
                preview,
                allow_session,
            })
            .await
        {
            Response::Allow => ApprovalDecision::AllowOnce,
            Response::AllowSession if allow_session => ApprovalDecision::AllowSession,
            Response::AllowSession => ApprovalDecision::AllowOnce,
            _ => ApprovalDecision::Deny,
        }
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
        self.decide_approval(
            format!("Allow {}? ({:?})", req.tool_name, req.risk),
            preview,
            req.risk != ToolRisk::Sensitive,
        )
        .await
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

    async fn decide_command(&self, req: ProviderCommandRequest) -> ApprovalDecision {
        self.decide_approval(
            "Allow command?".into(),
            format!(
                "{}\n{}\n{}",
                req.cwd.unwrap_or_default(),
                req.command,
                req.reason.unwrap_or_default()
            ),
            true,
        )
        .await
    }

    async fn decide_file_change(&self, req: ProviderFileChangeRequest) -> ApprovalDecision {
        let allow_session = req.path.as_deref().is_some_and(|p| {
            !p.is_empty() && tools::sensitive::write_risk(p) != ToolRisk::Sensitive
        });
        self.decide_approval(
            "Allow file change?".into(),
            format!(
                "{}\n{}\n{}",
                req.path.unwrap_or_default(),
                req.reason.unwrap_or_default(),
                req.diff.unwrap_or_default()
            ),
            allow_session,
        )
        .await
    }

    async fn answer_question(&self, req: ProviderQuestionRequest) -> Option<Vec<String>> {
        self.question(req.prompt, req.choices, req.multiple).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::StreamExt;

    #[tokio::test]
    async fn allow_session_grants_ordinary_requests_and_downgrades_sensitive_ones() {
        let (events, mut rx) = futures::channel::mpsc::unbounded();
        let host = Interactions {
            events,
            policy: Arc::new(Mutex::new(ApprovalPolicy::new(ApprovalMode::Manual))),
        };

        let answerer = tokio::spawn(async move {
            while let Some(ev) = rx.next().await {
                if let EvKind::Card(card) = ev.kind {
                    let _ = card.reply.send(Response::AllowSession);
                }
            }
        });

        let write_decision = host
            .decide(&ApprovalRequest {
                approval_id: "a1".into(),
                tool_name: "edit_file".into(),
                tool_call_id: "t1".into(),
                risk: ToolRisk::Write,
                preview: ApprovalPreview {
                    path: "src/lib.rs".into(),
                    summary: "edit src/lib.rs".into(),
                    diff: String::new(),
                },
            })
            .await;
        assert_eq!(write_decision, ApprovalDecision::AllowSession);

        let sensitive_decision = host
            .decide(&ApprovalRequest {
                approval_id: "a2".into(),
                tool_name: "edit_file".into(),
                tool_call_id: "t2".into(),
                risk: ToolRisk::Sensitive,
                preview: ApprovalPreview {
                    path: ".env".into(),
                    summary: "edit .env".into(),
                    diff: String::new(),
                },
            })
            .await;
        assert_eq!(
            sensitive_decision,
            ApprovalDecision::AllowOnce,
            "sensitive secret files must never acquire a session grant"
        );

        let cmd_decision = host
            .decide_command(ProviderCommandRequest {
                approval_id: "c1".into(),
                command: "cargo build".into(),
                cwd: Some(".".into()),
                reason: None,
            })
            .await;
        assert_eq!(cmd_decision, ApprovalDecision::AllowSession);

        let env_file_decision = host
            .decide_file_change(ProviderFileChangeRequest {
                approval_id: "f1".into(),
                path: Some(".env".into()),
                diff: None,
                reason: None,
            })
            .await;
        assert_eq!(env_file_decision, ApprovalDecision::AllowOnce);

        drop(host);
        let _ = answerer.await;
    }
}
