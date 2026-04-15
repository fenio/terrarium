use anyhow::Result;
use kube::config::{KubeConfigOptions, Kubeconfig};

pub struct ClusterInfo {
    pub context_name: String,
}

pub async fn create_client(context: Option<&str>) -> Result<(kube::Client, ClusterInfo)> {
    let client = if let Some(ctx) = context {
        let kubeconfig = Kubeconfig::read()?;
        let options = KubeConfigOptions {
            context: Some(ctx.to_string()),
            ..Default::default()
        };
        let config = kube::Config::from_custom_kubeconfig(kubeconfig, &options).await?;
        kube::Client::try_from(config)?
    } else {
        kube::Client::try_default().await?
    };

    let context_name = if let Some(ctx) = context {
        ctx.to_string()
    } else {
        Kubeconfig::read()
            .ok()
            .and_then(|kc| kc.current_context)
            .unwrap_or_else(|| "unknown".to_string())
    };

    Ok((client, ClusterInfo { context_name }))
}
