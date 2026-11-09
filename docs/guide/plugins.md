# Plugins

Every feature in Terrarium is a plugin. The app ships with the
[recommended plugins](../../README.md#recommended-plugins) (To-do, Notes,
Habits, Bookmarks, Voice and Reports), and you can add others. Manage them in
**Settings → Plugins**, or **Apps → Manage** on a phone.

## What a plugin can do

A plugin runs in a WebAssembly sandbox with its own database, and has no
network or file access. Anything beyond its own data is declared in its
manifest and shown to you before install: AI text generation, local
speech-to-text, scheduled jobs, reminders, data it shares with or uses from
other plugins, and raised resource limits. It draws its UI with declarative
templates, so it cannot run code in the app shell.

## Install or update

1. In **Install or update**, choose where the plugin comes from:
   - **Package file (.terrapkg)** – a signed package someone published.
   - **Built package folder** (desktop) – a plugin you built with `terra build`.
   - **Git repository** (desktop) – an `https://`, `ssh://` or `git@` address of
     a repository that contains a built package. Terrarium never builds code
     from Git.
2. Choose **Check package**. Terrarium verifies the package and shows its
   publisher and **This plugin can**, the permissions it asks for.
3. Tick **Allow the permissions listed above** and choose **Install**.

Installing a newer version of a plugin you already have updates it. If the new
version asks for anything new, you are asked again.

Unsigned packages install only in development builds.

## Other devices

Installed packages sync to your paired devices, but each device asks before
running a new plugin or version. Those wait under **Needs review** until you
approve them on that device.

## Roll back and remove

Each installed plugin can be rolled back to its previous version or removed.
Removing keeps the plugin's data; it is listed under **Removed**, and adding it
back restores it.

## Trusted publishers

Packages signed by a key in **Trusted publishers** install without asking
about the publisher. You are still asked about permissions.

## Write your own

See [writing a plugin](../develop/plugin-authoring.md). The `terra` CLI
scaffolds, builds, checks, hot-reloads, signs and previews plugins.
