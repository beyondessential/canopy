---
id: PRJ
---

# Projects view

Applies to the fleet's projects view: which groups it lists, how a person's own groups are set apart, what it shows of each group, and how it is searched.

## Where it is

The projects view is a listing of the fleet, beside its groups, archived records and figures (see [FLT](overview.md), "Navigating the two grains").
It is addressed at `/fleet/pm` and named "Projects" in the operator interface's navigation.
Its address is stable, since a tailnet policy can name it as a default homepage (see [HOME](../private-server/homepage.md)).
Everyone who can read the operator surface can read it (see [ADM](../private-server/access-levels.md)).

## Which groups it lists

The view lists every live group with a production environment (see [GRP](groups.md), "Environments").

> [!NOTE]
> The view is for reaching live Tamanu, so a project before go-live is kept on its group's page until the group has a production environment.

The groups whose PM in charge is the person viewing come first, in a section of their own, highlighted, and every other listed group follows in a second section (see [GRP](groups.md), "Project details").
A person is matched to a PM in charge by their tailnet login, ignoring case.
A section with no groups in it is not shown.
Within each section, groups are in order of name.

## What each group shows

Each group shows its name, leading to the group's page, and its project details: its project, its PM in charge and its contact on the ground.
A detail not filled in reads as none.

Beneath them, each group shows the Tamanu in its production environment: its central first, then each facility in order of name.
Each is named as everywhere in the fleet, so facilities sharing a name are told apart by their machine (see [FLT](overview.md), "Naming").
A production environment with neither a Tamanu central nor a facility reads as having no Tamanu.

Each has two addresses, each a link opening in a new tab:

- its public address, which is the application's configured URL;
- its tailnet address, which is the tailnet name of the device bound to its machine, linked over HTTPS at that MagicDNS name as Canopy's other links to a machine are (see [SVC](../private-server/service-links.md)).

Where either address is not known, it reads as none.
An application hosted on a cluster has no machine, and so no tailnet address.

> [!NOTE]
> Not every machine is on the tailnet, and not every application is configured with a public URL, so either address can be missing on its own.

## Searching

A search box narrows both sections to the groups whose name, project, PM in charge or contact on the ground contains what is typed, ignoring case.
A PM in charge matches by their display name as well as by their login or address.
