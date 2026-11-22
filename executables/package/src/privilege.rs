//! Who may change the installed packages, and how they get the rights to.
//!
//! The system package directories stay writable by root only. Root and the
//! members of the `administrator` group may still install, upgrade and remove
//! packages: after checking the invoking user, the `package` task becomes root
//! for the duration of the operation, then gets its identity back.
//!
//! The check covers the task and its parent, so a set-user-ID mount of the
//! executable does not let any user in: the invoking user must qualify too.

use alloc::{format, string::String};
use xila::{
    executable::Standard,
    task::TaskIdentifier,
    users::{GroupIdentifier, UserIdentifier},
};

/// Name of the group whose members may administer packages.
pub const ADMINISTRATOR_GROUP: &str = "administrator";

/// Whether `user` is root or a member of [`ADMINISTRATOR_GROUP`].
pub async fn is_privileged(users: &xila::users::Manager, user: UserIdentifier) -> bool {
    if user == UserIdentifier::ROOT {
        return true;
    }
    let Ok(groups) = users.get_user_groups(user).await else {
        return false;
    };
    for group in groups {
        if users
            .get_group_name(group)
            .await
            .is_ok_and(|name| name == ADMINISTRATOR_GROUP)
        {
            return true;
        }
    }
    false
}

/// The identity a task had before [`elevate`].
#[must_use = "restore the identity of the task once the operation ends"]
pub struct Elevation {
    task: TaskIdentifier,
    user: UserIdentifier,
    group: GroupIdentifier,
}

/// Check that the invoking user may administer packages, then run the task as root.
pub async fn elevate(standard: &Standard<'static>) -> Result<Elevation, String> {
    let context = standard.context;
    let tasks = context.task_manager;
    let task = tasks.get_current_task_identifier().await;
    let user = tasks
        .get_user(task)
        .await
        .map_err(|e| format!("cannot determine the user: {e}"))?;
    let group = tasks
        .get_group(task)
        .await
        .map_err(|e| format!("cannot determine the group: {e}"))?;
    let mut invokers = alloc::vec![user];
    if let Ok(parent) = tasks.get_parent(task).await
        && parent != task
        && let Ok(parent_user) = tasks.get_user(parent).await
    {
        invokers.push(parent_user);
    }
    for invoker in invokers {
        if !is_privileged(context.users_manager, invoker).await {
            return Err(format!(
                "changing packages requires root or the {ADMINISTRATOR_GROUP} group"
            ));
        }
    }
    let elevation = Elevation { task, user, group };
    tasks
        .set_user(task, UserIdentifier::ROOT)
        .await
        .map_err(|e| format!("cannot run as root: {e}"))?;
    if let Err(e) = tasks.set_group(task, GroupIdentifier::ROOT).await {
        elevation.restore(standard).await;
        return Err(format!("cannot run as root: {e}"));
    }
    Ok(elevation)
}

impl Elevation {
    /// Give the task its identity back.
    pub async fn restore(self, standard: &Standard<'static>) {
        let tasks = standard.context.task_manager;
        // The task exists (it is running this code), so these cannot fail.
        let _ = tasks.set_group(self.task, self.group).await;
        let _ = tasks.set_user(self.task, self.user).await;
    }
}
