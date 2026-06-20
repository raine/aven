# Queue view

The queue is the default attention view in the TUI. It answers:

> What should I look at next?

It is not a list of every task. Done and canceled tasks are kept out of the queue so the view stays focused on work that can still be picked up.

## Queue groups

Tasks are grouped from most urgent to least urgent:

1. **Needs action**
   - Tasks with conflicts
   - Urgent tasks
   - Active tasks that have not been updated for a week or more

2. **Focus**
   - Active tasks
   - High-priority todo tasks

3. **Triage**
   - Inbox tasks
   - Medium-priority todo tasks

4. **Later**
   - Backlog tasks
   - Low-priority todo tasks
   - Todo tasks with no priority

## Order inside each group

Inside a group, tasks are ordered by a local score. In simple terms:

1. More important statuses come first.
2. Higher priorities come first.
3. Stale active, todo, and inbox tasks move upward.
4. Older tasks win ties.

The score is deterministic and local. There is no LLM ranking or hidden service call.

## Age column

In the queue view, the `AGE` column shows how long it has been since the task was updated. This matches the staleness signal used by queue ordering.

In other views, `AGE` shows how long ago the task was created.

## Done and canceled work

Done tasks are available in the **Done** view in the sidebar.

Canceled tasks are not part of the queue. They can still be found by filtering for the canceled status.
