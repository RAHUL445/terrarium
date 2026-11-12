# Plugins

Every feature in Terrarium is a plugin. The app ships with the
[recommended plugins](../../README.md#recommended-plugins) (To-do, Notes,
Habits, Bookmarks, Voice and Reports), and you can add others. Manage them in
**Settings → Plugins**, or **Apps → Manage** on a phone.

## What a plugin can do

A plugin runs in a WebAssembly sandbox with its own database, and has no
ambient network or file access. Network access goes through the host and
requires permission for the websites shown during install. Anything beyond its own data is declared in its
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
   Network plugins use **Allow and install** or **Allow and update**; see
   [Network access](#network-access) for the additional website approval.

Installing a newer version of a plugin you already have updates it. If the new
version asks for anything new, you are asked again.

Unsigned packages install only in development builds.

## Network access

“Reach example.com” lets a plugin send requests to that website.
“Reach any subdomain of example.org” covers its subdomains, not example.org
itself. “Reach any website” covers any public HTTPS website, including sites
you have not added, and requires **I allow access to any website** before
**Allow and install** (or **Allow and update**).

A plugin can send data it can read to these websites. Only approve plugins
and destinations you trust. Local addresses and unencrypted HTTP are blocked;
typing a feed URL does not grant extra permission. A version that asks for
more websites asks for approval again on each device.

In **Settings → Plugins** (or **Apps → Manage** on a phone), find the installed
plugin and choose **Network activity**. It shows recent times, websites,
results and sent/received sizes on this device. It does not show URL paths,
search parameters, messages or keys, and does not sync. Choose **Clear network
activity** to clear the plugin's history. History is limited to 500 entries
and 1 MiB. Removing a plugin stops new requests; a request already sent cannot
be recalled, and bounded activity history is retained.

### Accounts

Under the plugin's **Accounts**, open **Add API key**. Enter an account name,
choose or enter the exact website that should receive it, paste a bearer API
key, then choose **Save API key**. The key is hidden and is not shown again.
Only that plugin in that space can use the account, for its listed websites.
The plugin cannot read the key itself. Trust the destination: it receives the
key and could send it back in a reply.

For a plugin offering browser sign-in, register a client with the provider,
then enter an account name, client ID and optional client secret. Review the
sign-in server, token server, API websites and requested permissions. Choose
**Sign in with …** and finish in your system browser; **Cancel** stops waiting.
Sign-in times out after five minutes. On a phone, sign in on your computer,
then sync and approve the account on the phone.

Accounts sync encrypted with your space, but a new or changed account needs
**Use this account on this device** before use on another device. Approve only
websites and accounts you recognize. “Sign in again on this device” means the
saved account cannot currently be used there. Paired devices share the key
that protects these accounts, so only pair devices you trust.

Choose **Sign out** for browser accounts or **Remove** for API keys. Removal
syncs to your other devices and cancels related work; removing the plugin
removes its accounts too. Revoke access at the provider to invalidate tokens
already issued. Old encrypted values may remain in backups or history.

### Feeds and phones

Feeds reads public HTTPS RSS and Atom feeds, with text summaries, read/unread
controls and an unread widget. It keeps the newest 200 items per feed plus older unread items, and
refreshes at most 20 feeds per run. Its default hourly schedule runs on your
computer. Feeds requests **Reach any website** so you can add different feed
sites; that permission also allows sites you have not entered. Images in feed
content are not fetched automatically. Links open in your system browser.

Network requests run only while the app is open; on Android, only while it is
in the foreground. Closing the process loses pending work, without automatic
replay. Browser preview supports public requests but has no accounts/sign-in;
headless serving does not send plugin requests. Upgrade all paired devices
before enabling network plugins.

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
