//! Rendering checks for hand-expanded `async_trait` signatures.

mod utils;

#[cfg(test)]
mod tests {
    use std::sync::OnceLock;

    use libruskel::Renderer;

    use super::utils::TestFixture;

    const SOURCE: &str = r#"
pub mod send_trait {
    pub struct Context<'a>(pub &'a str);

    pub trait Service {
        fn read<'life0, 'life1, 'async_trait>(
            &'life0 self,
            context: Context<'life1>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = String> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            'life1: 'async_trait,
            Self: Sync + 'async_trait;

        fn provided<'a, 'life0, 'async_trait>(
            &'life0 self,
            text: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = usize> + Send + 'async_trait>>
        where
            'a: 'async_trait,
            'life0: 'async_trait,
            Self: Sync + 'async_trait,
        {
            Box::pin(async move { text.len() })
        }
    }
}

pub mod local_trait {
    pub trait Service {
        fn run<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait;
    }
}

pub mod arc_receiver {
    pub trait Service {
        fn run<'async_trait>(
            self: std::sync::Arc<Self>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'async_trait>>
        where
            Self: Send + Sync + 'async_trait;
    }
}

pub mod mixed_policy {
    pub trait Service {
        fn send<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: Sync + 'async_trait;

        fn local<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: 'async_trait;
    }
}

pub mod near_match {
    pub trait Service {
        fn run<'life0, 'async_trait>(
            &'life0 self,
        ) -> Box<dyn std::future::Future<Output = ()> + Send + 'async_trait>
        where
            'life0: 'async_trait,
            Self: Sync + 'async_trait;
    }
}

pub mod send_impl {
    pub struct Worker;

    pub trait Service {
        fn run<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = u8> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: Sync + 'async_trait;
    }

    impl Service for Worker {
        fn run<'life0, 'async_trait>(
            &'life0 self,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = u8> + Send + 'async_trait>>
        where
            'life0: 'async_trait,
            Self: Sync + 'async_trait,
        {
            Box::pin(async { 7 })
        }
    }
}
"#;

    fn render_case(name: &str) -> String {
        static FIXTURE: OnceLock<TestFixture> = OnceLock::new();
        let fixture = FIXTURE.get_or_init(|| TestFixture::new(SOURCE));
        Renderer::default()
            .with_private_items(true)
            .render(&fixture.case(name))
            .expect("fixture renders")
    }

    #[test]
    fn send_trait_restores_methods_and_lifetimes() {
        let output = render_case("send_trait");
        assert!(output.contains("#[async_trait]"), "{output}");
        assert!(
            output.contains("async fn read(&self, context: Context<'_>) -> String"),
            "{output}"
        );
        assert!(
            output.contains("async fn provided<'a>(&self, text: &'a str) -> usize"),
            "{output}"
        );
        assert!(!output.contains("'async_trait"), "{output}");
        assert!(!output.contains("'life0"), "{output}");
        assert!(!output.contains("'life1"), "{output}");
    }

    #[test]
    fn local_trait_restores_unit_output() {
        let output = render_case("local_trait");
        assert!(output.contains("#[async_trait(?Send)]"), "{output}");
        assert!(output.contains("async fn run(&self);"), "{output}");
        assert!(!output.contains("'async_trait"), "{output}");
    }

    #[test]
    fn arc_receiver_restores_receiver() {
        let output = render_case("arc_receiver");
        assert!(output.contains("#[async_trait]"), "{output}");
        assert!(
            output.contains("async fn run(self: std::sync::Arc<Self>);"),
            "{output}"
        );
    }

    #[test]
    fn mixed_policy_stays_expanded() {
        let output = render_case("mixed_policy");
        assert!(!output.contains("#[async_trait"), "{output}");
        assert!(output.contains("'async_trait"), "{output}");
        assert!(!output.contains("async fn"), "{output}");
    }

    #[test]
    fn near_match_stays_expanded() {
        let output = render_case("near_match");
        assert!(!output.contains("#[async_trait"), "{output}");
        assert!(output.contains("'async_trait"), "{output}");
        assert!(!output.contains("async fn"), "{output}");
    }

    #[test]
    fn impl_and_trait_both_restore_methods() {
        let output = render_case("send_impl");
        assert_eq!(output.matches("#[async_trait]").count(), 2, "{output}");
        assert_eq!(
            output.matches("async fn run(&self) -> u8").count(),
            2,
            "{output}"
        );
        assert!(!output.contains("'async_trait"), "{output}");
    }
}
