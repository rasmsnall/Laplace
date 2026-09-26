use std::collections::HashMap;

use crate::config::Flow;
use crate::status::{State, Status};

/// a flow that depends on itself through `after`, if any
pub fn find_cycle(flows: &[Flow]) -> Option<String> {
    let mut waiting_on: HashMap<&str, usize> = flows
        .iter()
        .map(|flow| (flow.id.as_str(), flow.after.len()))
        .collect();
    let mut ready: Vec<&str> = waiting_on
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();

    while let Some(done) = ready.pop() {
        waiting_on.remove(done);
        for flow in flows
            .iter()
            .filter(|flow| flow.after.iter().any(|upstream| upstream == done))
        {
            if let Some(count) = waiting_on.get_mut(flow.id.as_str()) {
                *count -= 1;
                if *count == 0 {
                    ready.push(&flow.id);
                }
            }
        }
    }
    waiting_on.into_keys().min().map(str::to_owned)
}

/// a late or pending step whose upstream is broken is blocked rather than late,
/// so only the first broken step in a pipeline alerts. `statuses` is in the same order as `flows`.
pub fn block_downstream(flows: &[Flow], statuses: &mut [Status]) {
    let index: HashMap<&str, usize> = flows
        .iter()
        .enumerate()
        .map(|(i, flow)| (flow.id.as_str(), i))
        .collect();

    // blocking travels one step per pass, so a chain of n flows settles within n passes
    for _ in 0..flows.len() {
        let mut changed = false;
        for (i, flow) in flows.iter().enumerate() {
            if !matches!(statuses[i].state, State::Late | State::Pending) {
                continue;
            }
            let broken = flow
                .after
                .iter()
                .filter_map(|upstream| index.get(upstream.as_str()).copied())
                .find(|&j| statuses[j].state.is_broken());
            if let Some(j) = broken {
                statuses[i].detail = format!(
                    "waiting on {} ({})",
                    flows[j].id,
                    statuses[j].state.as_str()
                );
                statuses[i].state = State::Blocked;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{PushKind, placeholder};

    fn flow(id: &str, after: &[&str]) -> Flow {
        Flow {
            id: id.into(),
            after: after.iter().map(|a| a.to_string()).collect(),
            ..placeholder(PushKind::Heartbeat)
        }
    }

    fn status(id: &str, state: State) -> Status {
        Status {
            id: id.into(),
            kind: "heartbeat",
            source: String::new(),
            every: String::new(),
            after: Vec::new(),
            owner: None,
            acknowledged: None,
            silenced_until: None,
            silence_note: None,
            state,
            detail: String::new(),
            last_ok: None,
            host_key_pending: false,
            alerted_state: String::new(),
            recorded_state: None,
        }
    }

    #[test]
    fn finds_circles_but_not_chains() {
        assert_eq!(
            find_cycle(&[flow("a", &[]), flow("b", &["a"]), flow("c", &["b"])]),
            None
        );
        assert!(find_cycle(&[flow("a", &["c"]), flow("b", &["a"]), flow("c", &["b"])]).is_some());
    }

    #[test]
    fn blocks_down_the_chain_but_keeps_real_failures() {
        let flows = [
            flow("gcs", &[]),
            flow("adls", &["gcs"]),
            flow("load", &["adls"]),
            flow("report", &["adls"]),
        ];
        let mut statuses = [
            status("gcs", State::Failed),
            status("adls", State::Late),
            status("load", State::Pending),
            status("report", State::Failed),
        ];
        block_downstream(&flows, &mut statuses);

        assert!(statuses[0].state == State::Failed);
        assert!(
            statuses[1].state == State::Blocked && statuses[1].detail == "waiting on gcs (failed)"
        );
        assert!(
            statuses[2].state == State::Blocked
                && statuses[2].detail == "waiting on adls (blocked)"
        );
        assert!(
            statuses[3].state == State::Failed,
            "its own failure stays visible"
        );
    }
}
