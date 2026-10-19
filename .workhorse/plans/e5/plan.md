# Sidebar navigation

The top app bar and the per-section tab bars (Fleet, Devices, Settings) become one sidebar, following the app sidebar mockups on this card.

## Build

- [x] Sidebar component: sections, expandable subpages with Settings headings, incidents count, external links
- [x] Mode band at the sidebar foot, expanding upwards inline; the pill control stays for the narrow top bar
- [x] App shell: permanent drawer at `md` and up, top bar with menu button and temporary drawer below it; banners move into the content column
- [x] Drop the tab bars from Fleet, Devices, and Settings
- [x] Update e2e tests that drove the tabs, and add sidebar coverage
- [x] Narrow top bar: incidents link beside the mode control, which is a full-height square block dropping a menu at its own width
