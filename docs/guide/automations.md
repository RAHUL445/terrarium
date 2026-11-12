# Automations

Automations connect an event in one app to an action in another, within the
same space. For example, finishing a To-do task tagged gym can check in your Gym
habit. Time-based actions still belong in [Scheduled](ai-and-schedules.md).

## Create an automation

1. Open **Settings → Automations** in the space you want to use.
2. Choose the device that will run automations. On that device, confirm starting
   them, or use **Start on this device** when offered. Nothing runs until you
   choose and start a device. Phones run automations only while the app is open.
3. Create an automation with a name, one event and one action. Add up to ten
   conditions; all must match. Choose a **Fixed value** or **From the event**
   for each action argument. Only compatible event fields are offered.
4. Save, then review **Allow this automation?** on the chosen device. **Not now**
   leaves it saved without permission to run. Only new events will run after
   approval; old completions are not replayed.

The Gym example is a starting point, not an active rule. Select the real Gym
habit's id, use the task's completed date as the check-in day, and set the status
to done. The gym tag is used only to check the condition; the task title is not
passed to Habits. Complete a new matching task to try it, then check the habit
and automation history. A task without the tag should do nothing.

## Review what is shared

The consent sheet shows the event, every condition, the target action, fixed
values, fields passed between apps, fields used only for conditions, and the
execution device. Review those details before choosing **Allow**. Event data is
also synced to paired devices even when no automation uses it; plugin install
and update consent discloses that sharing. Only mapped fields reach the action.

**Needs your OK on this device** means this device has no approval for the
current rule and plugin versions. Approvals stay on each device. After moving
execution, review again there and start automations there. Editing the event,
conditions, action or arguments, or updating either plugin, requires review
again. Renaming alone keeps approval. Turning a rule off clears its local
permission to run; turn it on locally again. A change synced from another device
cannot grant permission or silently restart paused work.

## Choose and pause a device

The execution device is chosen per space. There is no automatic failover. When
it is offline, other devices wait; events can queue for up to seven days. The
space must be open on the execution device. If the chosen device is removed or
blocked, choose a replacement, review its rules and start it.

Use **Pause all automations** to pause the space, or turn off an individual
rule. After unpausing, confirm starting on the execution device if asked. When
changing devices, bring the old one online to receive the change: an offline
device may still act on its old settings and cause duplicates. Pausing is not
an immediate stop on disconnected devices.

## Understand history and problems

History shows the time, automation, device and Done, Failed, Skipped or Stopped
status, without event values. Use the history paging controls for older runs.
The newest 50 runs per automation are retained for at most 30 days. Deleted
rules' history expires after 30 days. Events expire after seven days, including
ones still waiting. This removes them from active history, but old encrypted
sync files and deleted records can remain on disk.

**Stopped** means a safety limit prevented the action, such as too many
automations in a row or too many actions from one event. Chains stop at three
event levels beyond the original event; at most ten matching actions run for
one event. Hourly limits are 60 attempts per automation and 600 across this
installation. Rate-limited matches are skipped and consumed, not queued for a
later burst. Simplify a loop or reduce the triggering activity before trying a
new event.

For **Failed**, read the fixed reason, check plugin availability and review any
changed rule. Failed and interrupted actions are never retried automatically;
fix the cause and perform a missed action manually. A crash after recording an
attempt can leave it reported as interrupted before it finished, even if its action never ran.
A crash between saving a plugin change and recording its event can lose that
event. Attempts are at most once per device, converging across devices into one
history entry; moving devices while offline can still repeat an action.

If nothing runs, check the selected device is online with the space open,
automations are started and unpaused there, the rule is on and approved there,
and a new event matches every condition. Expired events and events older than
approval do not run. A device clock over five minutes ahead delays its events
until the clock catches up. After restoring onto a new installation, review and
start locally again; do not copy another device's approval or authority files.

**Automation history is full** means this installation reached its 100,000 local execution
record limit. New actions stop to preserve protection against replay. There is
no supported automatic cleanup of that protection; do not delete local authority
files to bypass it.

The headless `terrarium serve` process can record events but does not execute
automations. Developer preview can test them with an isolated local identity,
after explicitly choosing, reviewing and starting there. CLI and MCP clients
can read rules and history but cannot create, approve or start automations.

Only install plugins and pair devices you trust. A compromised paired device
can forge an event that triggers an already approved automation, suppress work
or falsify synced history. See the [threat model](../security/threat-model.md)
for this boundary and [sync and devices](sync-and-devices.md) for pairing.
