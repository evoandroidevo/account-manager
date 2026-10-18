use lettre::{
    AsyncFileTransport, AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor,
    message::{Mailbox, header::ContentType},
    transport::smtp::authentication::Credentials,
};

use crate::config::{MailConfig, MailTransport};

#[derive(Debug, thiserror::Error)]
pub enum MailError {
    #[error("invalid mail address: {0}")]
    Address(#[from] lettre::address::AddressError),
    #[error("could not build message: {0}")]
    Build(#[from] lettre::error::Error),
    #[error("SMTP delivery failed: {0}")]
    Smtp(#[from] lettre::transport::smtp::Error),
    #[error("writing to the outbox failed: {0}")]
    File(#[from] lettre::transport::file::Error),
    #[error("could not create the outbox directory: {0}")]
    Io(#[from] std::io::Error),
}

/// Sends plain-text notification mail through the configured transport.
#[derive(Clone)]
pub struct Mailer {
    from: Mailbox,
    transport: Transport,
}

#[derive(Clone)]
enum Transport {
    Smtp(AsyncSmtpTransport<Tokio1Executor>),
    File(AsyncFileTransport<Tokio1Executor>),
    Log,
}

impl std::fmt::Debug for Mailer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let transport = match self.transport {
            Transport::Smtp(_) => "smtp",
            Transport::File(_) => "file",
            Transport::Log => "log",
        };
        f.debug_struct("Mailer")
            .field("from", &self.from.to_string())
            .field("transport", &transport)
            .finish()
    }
}

impl Mailer {
    pub fn new(config: &MailConfig) -> Result<Self, MailError> {
        let from = config.from.parse::<Mailbox>()?;
        let transport = match config.transport {
            MailTransport::Smtp => {
                let smtp = &config.smtp;
                let builder = if smtp.starttls {
                    AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&smtp.host)?
                } else {
                    AsyncSmtpTransport::<Tokio1Executor>::relay(&smtp.host)?
                };
                let mut builder = builder.port(smtp.port);
                if !smtp.username.is_empty() {
                    builder = builder.credentials(Credentials::new(
                        smtp.username.clone(),
                        smtp.password.clone(),
                    ));
                }
                Transport::Smtp(builder.build())
            }
            MailTransport::File => {
                std::fs::create_dir_all(&config.outbox_dir)?;
                Transport::File(AsyncFileTransport::<Tokio1Executor>::new(&config.outbox_dir))
            }
            MailTransport::Log => Transport::Log,
        };
        Ok(Self { from, transport })
    }

    pub async fn send(&self, to: &str, subject: &str, body: String) -> Result<(), MailError> {
        let message = Message::builder()
            .from(self.from.clone())
            .to(to.parse::<Mailbox>()?)
            .subject(subject)
            .header(ContentType::TEXT_PLAIN)
            .body(body)?;

        match &self.transport {
            Transport::Smtp(smtp) => {
                smtp.send(message).await?;
            }
            Transport::File(file) => {
                file.send(message).await?;
            }
            Transport::Log => {
                log::info!("mail transport is 'log': not sending \"{subject}\" to {to}");
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::config::{MailConfig, MailTransport};

    use super::Mailer;

    #[tokio::test]
    async fn file_transport_writes_an_eml_file() {
        let dir = std::env::temp_dir().join(format!("am-outbox-{}", uuid::Uuid::new_v4()));
        let mailer = Mailer::new(&MailConfig {
            transport: MailTransport::File,
            outbox_dir: dir.to_string_lossy().into_owned(),
            ..MailConfig::default()
        })
        .expect("mailer should build");

        mailer
            .send("alice@example.com", "Hello", "Body text".to_owned())
            .await
            .expect("mail should be written");

        let files: Vec<_> = std::fs::read_dir(&dir)
            .expect("outbox should exist")
            .map(|entry| entry.expect("entry").path())
            .collect();
        assert_eq!(files.len(), 1);
        let contents = std::fs::read_to_string(&files[0]).expect("eml should be readable");
        assert!(contents.contains("To: alice@example.com"));
        assert!(contents.contains("Subject: Hello"));
        assert!(contents.contains("Body text"));

        std::fs::remove_dir_all(dir).expect("outbox should be removable");
    }

    #[test]
    fn rejects_an_invalid_from_address() {
        let result = Mailer::new(&MailConfig {
            from: "not an address".to_owned(),
            ..MailConfig::default()
        });
        assert!(result.is_err());
    }
}
