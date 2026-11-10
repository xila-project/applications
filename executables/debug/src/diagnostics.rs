use alloc::{
    collections::BTreeSet,
    string::{String, ToString},
    vec::Vec,
};
use xila::task::{self, TaskIdentifier};

pub const MAX_DIAGNOSTIC_TASKS: usize = 64;

pub(crate) async fn diagnostics(
    context: &'static xila::executable::ExecutableContext,
) -> Result<serde_json::Value, (&'static str, String)> {
    let memory = xila::memory::global_manager();
    let (tasks, truncated) = collect_tasks(context.task_manager).await?;
    Ok(serde_json::json!({
        "tasks": tasks,
        "truncated": truncated,
        "memory": {
            "total_bytes": memory.get_total_size(),
            "used_bytes": memory.get_used(),
            "free_bytes": memory.get_free(),
            "page_size": memory.get_page_size(),
        }
    }))
}

trait TaskSource {
    async fn children(&self, task: TaskIdentifier) -> Result<Vec<TaskIdentifier>, task::Error>;
    async fn name(&self, task: TaskIdentifier) -> Result<String, task::Error>;
    async fn parent(&self, task: TaskIdentifier) -> Result<TaskIdentifier, task::Error>;
}

impl TaskSource for task::Manager {
    async fn children(&self, task: TaskIdentifier) -> Result<Vec<TaskIdentifier>, task::Error> {
        self.get_children(task).await
    }

    async fn name(&self, task: TaskIdentifier) -> Result<String, task::Error> {
        self.get_name(task).await
    }

    async fn parent(&self, task: TaskIdentifier) -> Result<TaskIdentifier, task::Error> {
        self.get_parent(task).await
    }
}

fn enqueue_task(
    task: TaskIdentifier,
    discovered: &mut BTreeSet<TaskIdentifier>,
    stack: &mut Vec<TaskIdentifier>,
    truncated: &mut bool,
) {
    if discovered.contains(&task) {
        return;
    }
    if discovered.len() >= MAX_DIAGNOSTIC_TASKS {
        *truncated = true;
        return;
    }
    discovered.insert(task);
    stack.push(task);
}

async fn collect_tasks(
    source: &impl TaskSource,
) -> Result<(Vec<serde_json::Value>, bool), (&'static str, String)> {
    let root = task::Manager::ROOT_TASK_IDENTIFIER;
    let initial_children = source
        .children(root)
        .await
        .map_err(|error| ("diagnostics_failed", error.to_string()))?;

    let mut discovered = BTreeSet::new();
    let mut stack = Vec::with_capacity(initial_children.len().min(MAX_DIAGNOSTIC_TASKS));
    let mut truncated = false;

    // Admit the root before a wide child list can consume the bounded budget.
    if source.name(root).await.is_ok() {
        enqueue_task(root, &mut discovered, &mut stack, &mut truncated);
    }
    for child in initial_children {
        enqueue_task(child, &mut discovered, &mut stack, &mut truncated);
    }

    let mut tasks = Vec::with_capacity(discovered.len().min(MAX_DIAGNOSTIC_TASKS));
    while let Some(task) = stack.pop() {
        if tasks.len() >= MAX_DIAGNOSTIC_TASKS {
            truncated = true;
            break;
        }
        let name = source.name(task).await.unwrap_or_else(|_| "unknown".into());
        let children = source.children(task).await.unwrap_or_default();
        for child in children {
            enqueue_task(child, &mut discovered, &mut stack, &mut truncated);
        }
        let parent = source.parent(task).await.map(usize::from).ok();
        tasks.push(serde_json::json!({
            "id": usize::from(task),
            "parent_id": parent,
            "name": name
        }));
    }
    Ok((tasks, truncated || !stack.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::{collections::BTreeMap, format};

    #[derive(Default)]
    struct MockTaskSource {
        names: BTreeMap<TaskIdentifier, String>,
        parents: BTreeMap<TaskIdentifier, TaskIdentifier>,
        children: BTreeMap<TaskIdentifier, Vec<TaskIdentifier>>,
    }

    impl MockTaskSource {
        fn with_root() -> Self {
            let mut source = Self::default();
            source.insert(
                task::Manager::ROOT_TASK_IDENTIFIER,
                task::Manager::ROOT_TASK_IDENTIFIER,
                "Root",
            );
            source
        }

        fn insert(&mut self, id: TaskIdentifier, parent: TaskIdentifier, name: &str) {
            self.names.insert(id, name.into());
            self.parents.insert(id, parent);
            self.children.entry(parent).or_default().push(id);
        }
    }

    impl TaskSource for MockTaskSource {
        async fn children(&self, task: TaskIdentifier) -> Result<Vec<TaskIdentifier>, task::Error> {
            Ok(self.children.get(&task).cloned().unwrap_or_default())
        }

        async fn name(&self, task: TaskIdentifier) -> Result<String, task::Error> {
            self.names
                .get(&task)
                .cloned()
                .ok_or(task::Error::InvalidTaskIdentifier)
        }

        async fn parent(&self, task: TaskIdentifier) -> Result<TaskIdentifier, task::Error> {
            self.parents
                .get(&task)
                .copied()
                .ok_or(task::Error::InvalidTaskIdentifier)
        }
    }

    #[test]
    fn root_self_parent_cycle_emits_unique_records_including_root_once() {
        let mut source = MockTaskSource::with_root();
        let root = task::Manager::ROOT_TASK_IDENTIFIER;
        let child = TaskIdentifier::new(1);
        let grandchild = TaskIdentifier::new(2);
        source.insert(child, root, "Shell");
        source.insert(grandchild, child, "Debug");

        let (tasks, truncated) = xila::task::block_on(collect_tasks(&source)).unwrap();
        assert!(!truncated);
        assert_eq!(tasks.len(), 3);

        let ids: Vec<usize> = tasks
            .iter()
            .map(|entry| entry["id"].as_u64().unwrap() as usize)
            .collect();
        let unique: BTreeSet<usize> = ids.iter().copied().collect();
        assert_eq!(ids.len(), unique.len(), "task IDs must be unique: {ids:?}");
        assert!(unique.contains(&usize::from(root)));
        assert!(unique.contains(&usize::from(child)));
        assert!(unique.contains(&usize::from(grandchild)));

        let root_record = tasks
            .iter()
            .find(|entry| entry["id"] == usize::from(root))
            .unwrap();
        assert_eq!(root_record["parent_id"], usize::from(root));
        assert_eq!(root_record["name"], "Root");
    }

    #[test]
    fn includes_root_when_children_list_omits_root_self_edge() {
        let mut source = MockTaskSource::default();
        let root = task::Manager::ROOT_TASK_IDENTIFIER;
        let child = TaskIdentifier::new(1);
        source.names.insert(root, "Root".into());
        source.parents.insert(root, root);
        source.insert(child, root, "Child");

        let (tasks, truncated) = xila::task::block_on(collect_tasks(&source)).unwrap();
        assert!(!truncated);
        assert_eq!(tasks.len(), 2);
        let ids: BTreeSet<usize> = tasks
            .iter()
            .map(|entry| entry["id"].as_u64().unwrap() as usize)
            .collect();
        assert_eq!(ids, BTreeSet::from([usize::from(root), usize::from(child)]));
    }

    #[test]
    fn truncation_boundary_at_64_and_65_unique_tasks() {
        let root = task::Manager::ROOT_TASK_IDENTIFIER;

        let mut source_64 = MockTaskSource::with_root();
        for index in 1..64u16 {
            source_64.insert(
                TaskIdentifier::new(index.into()),
                root,
                &format!("Task {index}"),
            );
        }
        let (tasks_64, truncated_64) = xila::task::block_on(collect_tasks(&source_64)).unwrap();
        assert_eq!(tasks_64.len(), 64);
        assert!(
            !truncated_64,
            "64 unique tasks should fit within the 64-task cap without truncation"
        );

        let mut source_65 = MockTaskSource::with_root();
        for index in 1..65u16 {
            source_65.insert(
                TaskIdentifier::new(index.into()),
                root,
                &format!("Task {index}"),
            );
        }
        let (tasks_65, truncated_65) = xila::task::block_on(collect_tasks(&source_65)).unwrap();
        assert_eq!(tasks_65.len(), 64);
        assert!(
            truncated_65,
            "the 65th unique task must report truncated: true even when it is the last pending stack entry"
        );
    }

    #[test]
    fn deep_chain_truncation_at_64_and_65_unique_tasks() {
        let mut source_64 = MockTaskSource::with_root();
        let mut parent = task::Manager::ROOT_TASK_IDENTIFIER;
        for index in 1..64u16 {
            let current = TaskIdentifier::new(index.into());
            source_64.insert(current, parent, &format!("Chain {index}"));
            parent = current;
        }
        let (tasks_64, truncated_64) = xila::task::block_on(collect_tasks(&source_64)).unwrap();
        assert_eq!(tasks_64.len(), 64);
        assert!(!truncated_64);

        let mut source_65 = source_64;
        source_65.insert(TaskIdentifier::new(64), parent, "Chain 64");
        let (tasks_65, truncated_65) = xila::task::block_on(collect_tasks(&source_65)).unwrap();
        assert_eq!(tasks_65.len(), 64);
        assert!(
            truncated_65,
            "deep chain with 65 unique tasks must report truncated: true when the 64th task discovers a 65th child"
        );
    }

    #[test]
    fn arbitrary_task_cycles_do_not_duplicate_or_falsely_truncate() {
        let mut source = MockTaskSource::with_root();
        let root = task::Manager::ROOT_TASK_IDENTIFIER;
        let first = TaskIdentifier::new(1);
        let second = TaskIdentifier::new(2);
        source.insert(first, root, "First");
        source.insert(second, first, "Second");
        // Introduce a synthetic cycle: first -> second -> first and second -> root.
        source.children.entry(second).or_default().push(first);
        source.children.entry(second).or_default().push(root);

        let (tasks, truncated) = xila::task::block_on(collect_tasks(&source)).unwrap();
        assert!(!truncated);
        assert_eq!(tasks.len(), 3);

        // Even at exactly 64 unique tasks, a cycle back to an already-discovered task
        // must not trigger truncation.
        let mut full_cycle_source = MockTaskSource::with_root();
        for index in 1..64u16 {
            full_cycle_source.insert(
                TaskIdentifier::new(index.into()),
                root,
                &format!("Task {index}"),
            );
        }
        full_cycle_source
            .children
            .entry(TaskIdentifier::new(63))
            .or_default()
            .push(root);
        full_cycle_source
            .children
            .entry(TaskIdentifier::new(1))
            .or_default()
            .push(TaskIdentifier::new(63));

        let (full_tasks, full_truncated) =
            xila::task::block_on(collect_tasks(&full_cycle_source)).unwrap();
        assert_eq!(full_tasks.len(), 64);
        assert!(
            !full_truncated,
            "cycles to already-discovered tasks at the 64-task cap must not set truncated: true"
        );
    }
}
