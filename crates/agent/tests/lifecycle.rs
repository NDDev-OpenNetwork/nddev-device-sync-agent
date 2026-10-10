#![cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use nddev_device_sync_agent::{
    NativeAgent,
    view::{AgentError, Reachability},
};

#[tokio::test]
async fn discovery_never_claims_observation_and_closed_hosts_refuse_work() {
    let agent = NativeAgent::discover().unwrap();
    let modules = agent.modules();
    assert_eq!(modules.len(), 6);
    assert!(
        modules.iter().all(|module| module.observed_at_ms.is_none()
            && module.reachability == Reachability::NotChecked)
    );
    agent.close();
    assert!(matches!(
        agent.system_info().await,
        Err(AgentError::Cancelled)
    ));
    assert!(matches!(
        agent.remote_sessions().await,
        Err(AgentError::Cancelled)
    ));
    assert!(
        agent
            .modules()
            .iter()
            .all(|module| module.observed_at_ms.is_none())
    );
}

#[tokio::test]
#[ignore = "requires scripts/check-agent.py with its paused native provider"]
async fn closing_an_in_flight_query_preserves_unobserved_state() {
    use std::time::Duration;
    assert_eq!(std::env::var("NDS_PAUSED_PROVIDER").as_deref(), Ok("1"));
    let agent = NativeAgent::discover().unwrap();
    let query = agent.system_info();
    tokio::pin!(query);
    tokio::select! {
        _ = &mut query => panic!("paused native provider completed before cancellation"),
        _ = tokio::time::sleep(Duration::from_millis(100)) => {}
    }
    agent.close();
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(7), query)
            .await
            .unwrap(),
        Err(AgentError::Cancelled)
    ));
    let state = agent
        .modules()
        .into_iter()
        .find(|module| module.id == "sysinfo")
        .unwrap();
    assert!(state.configured);
    assert_eq!(state.reachability, Reachability::NotChecked);
    assert!(state.observed_at_ms.is_none() && state.error.is_none());
}

#[tokio::test]
#[ignore = "requires scripts/check-agent.py with an empty child PATH"]
async fn cached_missing_provider_is_not_a_fresh_native_observation() {
    assert_eq!(std::env::var("PATH").as_deref(), Ok(""));
    let agent = NativeAgent::discover().unwrap();
    assert!(matches!(
        agent.remote_sessions().await,
        Err(AgentError::Unavailable)
    ));
    let state = agent
        .modules()
        .into_iter()
        .find(|module| module.id == "rds")
        .unwrap();
    assert!(!state.configured);
    assert_eq!(state.reachability, Reachability::NotChecked);
    assert!(state.observed_at_ms.is_none() && state.error.is_none());
}

#[tokio::test]
#[ignore = "requires scripts/check-agent.py with the real isolated sysinfo provider"]
async fn native_query_updates_only_its_own_observation() {
    assert_eq!(std::env::var("NDS_ISOLATED_PROVIDER").as_deref(), Ok("1"));
    let agent = NativeAgent::discover().unwrap();
    agent.system_info().await.unwrap();
    for module in agent.modules() {
        if module.id == "sysinfo" {
            assert_eq!(module.reachability, Reachability::Reachable);
            assert!(module.observed_at_ms.is_some() && module.error.is_none());
        } else {
            assert_eq!(module.reachability, Reachability::NotChecked);
        }
    }
    let observed = agent
        .modules()
        .into_iter()
        .find(|module| module.id == "sysinfo")
        .unwrap()
        .observed_at_ms;
    agent.close();
    assert!(matches!(
        agent.system_info().await,
        Err(AgentError::Cancelled)
    ));
    let latest = agent
        .modules()
        .into_iter()
        .find(|module| module.id == "sysinfo")
        .unwrap();
    assert_eq!(latest.reachability, Reachability::Reachable);
    assert_eq!(latest.observed_at_ms, observed);
}
