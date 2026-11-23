# 21. Keep the screen awake

## Outcome

A cook with the day's page open on a phone or tablet in the kitchen can follow a recipe without the screen going to sleep. A Keep awake setting in the footer, beside the theme toggle, offers Off and On. With On, a day page holds a screen wake lock while it's visible. On a phone or tablet, turning the device to landscape switches Keep awake to On and turning it back to portrait switches it to Off, so propping the device sideways in a stand is enough. A tap on Off or On overrides that until the next rotation.

Depends on: [14. Redesign](014-redesign.md).

## Scope

This adds a Keep awake group beside the theme toggle in the footer of day pages: the week page's day view and the missing-plan page, the two pages built on the day row. Week-list, year, household and invalid pages don't show the group, never hold a lock, and their footers don't change. Routes, statuses, the page JSON, the API, the MCP tools and the database don't change.

The [Screen Wake Lock API](https://developer.mozilla.org/en-US/docs/Web/API/Screen_Wake_Lock_API) (`navigator.wakeLock`) is exposed only in secure contexts: HTTPS, or `localhost`. Over plain HTTP the browser doesn't offer it, so the group isn't shown and no lock is held.

Out of scope: a wake lock on other pages, orientation switching on laptops and desktops, a setting that turns orientation switching off, a timer that releases the lock after a while, an indicator of whether a lock is currently held, a message when the browser refuses a lock, and locking the screen orientation.

## Behaviour

### Devices

- A **touch device** is one whose primary pointer is coarse (`matchMedia("(pointer: coarse)")` matches): a phone or tablet. Every other device is a **desktop**, such as a laptop. Sous reads this when the page loads.
- **Orientation** follows the viewport: landscape when `matchMedia("(orientation: landscape)")` matches, portrait otherwise. A tablet in a split-screen pane that's taller than wide counts as portrait.

### The choice

- The choice is Off or On, and the radio group always shows the choice in effect.
- A tap on an option is stored in the browser's local storage under `sous-keep-awake`, next to `sous-theme` and `sous-shopping-ticks`, as a JSON object with the choice and the orientation it was made in:
  `{ "on": true, "orientation": "portrait" }`
- **On a desktop**, the choice is the stored `on`, or Off when nothing is stored. Orientation is ignored. A desktop's choice behaves like the theme choice: set once, kept until changed.
- **On a touch device:**
  - When the page loads, the stored choice applies if its `orientation` equals the current orientation. Otherwise, or when nothing is stored, the choice is On in landscape and Off in portrait. So a tap holds across day pages until the device is turned.
  - When the orientation changes while the page is open, the choice becomes On for landscape and Off for portrait, and the stored key is removed. The tap's override ends with the rotation.
  - A tap on Off in landscape turns the lock off until the next rotation into landscape. A tap on On in portrait keeps it on until the next rotation.
- A stored value that isn't such an object, or has a missing or wrong-typed field, is treated as nothing stored and replaced on the next tap.
- If local storage is unavailable, unreadable or full, choices and rotations still apply to the open page, and no error is shown.
- The group is rendered only when `"wakeLock" in navigator`. Where it isn't, the group and its dot are absent, the footer looks as it does today, the stored choice is left alone, and nothing reacts to rotation.

### Holding the lock

A day page wants the lock while the choice is On and the page is visible (`document.visibilityState` is `"visible"`). Sous checks this when the page loads, when the choice changes (by a tap or a rotation), and when the visibility changes.

- When the lock becomes wanted and none is held, Sous calls `navigator.wakeLock.request("screen")`. It keeps the returned sentinel and listens for its `release` event, so it knows when the browser has released the lock. The browser does this when the page is hidden, for example on switching tabs or locking the phone, and Sous asks again when the page is visible.
- When the choice becomes Off, Sous releases a held lock (`sentinel.release()`) and stops all further requests.
- If a request is rejected, for example because battery saver is on or the browser wants a user gesture, the choice stays On, nothing is shown, and no error is logged. While the lock is still wanted, Sous asks again on the next visibility change to visible or the next click anywhere on the page, whichever comes first. A click never sends a request while a lock is held or while the choice is Off.
- Only one request is in flight at a time. A load, a rotation, a visibility change and a click arriving together make one request, not several. If the choice becomes Off while a request is in flight, the sentinel it returns is released at once.
- Leaving the page, by a link or the day steppers, is a full navigation: the old page's lock goes with it, and the new day page decides again from storage and orientation.

## Layout and styles

- On day pages, the footer's theme row holds two groups on one line: the Theme radio group, a middle dot, then the Keep awake radio group. The groups are centred as one line, and the dot sits `space-4` from each group.
- The dot is `·` in `small` `ink-muted`, hidden from assistive technology, like the `Week 46 · 2026` separator.
- The Keep awake group copies the Theme group exactly. It's a `role="radiogroup"` labelled by a `KEEP AWAKE` `label` in `ink-muted`, then the options Off and On in `small`. Each option is a native radio inside a `<label>` with `name="keep-awake"`. Unchosen options are `ink-muted` and turn `ink` on hover. The chosen one is `ink` with a solid 1.5px underline at a 5px offset. Keyboard focus shows the standard 2px `focus` outline at a 2px offset. Arrow keys move between the two options, as in any native radio group. A rotation moves the checked radio without moving focus.
- When the two groups don't fit on one line, the Keep awake group moves to its own centred line below Theme, the footer's `space-4` gap apart, and the dot is hidden. Use a container query on the footer, not a JavaScript measurement, so the layout is right on first paint. The breakpoint is the narrowest footer width where both groups and the dot fit on one line with Jost loaded. Measure it, pin it as a constant, and record it in the design system. At 1280 px the groups share one line.
- It reuses the theme group's classes, or shared ones it's refactored into. It adds no new token, colour or shape, and has no icon: the words carry it. Pages without the Keep awake group keep today's single Theme group with no dot.

## Documentation

- [Design system](../design-system.md), written by one owner:
  - **Content:** add "Keep awake", "Off" and "On" to the app's own words.
  - **Theme toggle:** on day pages, a Keep awake group follows the Theme group in the same row, after a middle dot `·` in `ink-muted`. It's styled like Theme, with options Off and On. On keeps the screen awake while the page is visible. On a phone or tablet, landscape switches it On and portrait switches it Off, and a tap holds until the next rotation. The choice is kept on the device under `sous-keep-awake`. Below the pinned footer width, Keep awake wraps under Theme and the dot is hidden. The group is absent where the browser can't hold a wake lock.
  - **Type, `label`:** add "Keep awake" to its uses.
  - The day page build order: the footer's theme row holds the Theme and Keep awake groups.
- [Technical design](../technical-design.md):
  - The UI state paragraph: the keep-awake choice is the third piece of kept UI state, stored under `sous-keep-awake` with the orientation it was made in. With On, day pages hold a screen wake lock while visible. On coarse-pointer devices, orientation sets the choice: landscape On, portrait Off, and a tap overrides until the next rotation. Nothing is sent to the server.
  - Under intentional changes, next to the theme toggle line: day pages add a Keep awake setting, Off or On, that keeps the screen awake where the browser supports the Screen Wake Lock API, and that turns On in landscape on phones and tablets.

## Acceptance criteria

1. On day and missing-plan pages, when `navigator.wakeLock` exists, the footer's theme row shows the Theme group, a middle dot and a Keep awake radio group with Off and On. Week-list, year, household and invalid pages show no Keep awake group and no dot.
2. When `navigator.wakeLock` doesn't exist, no Keep awake group or dot is rendered, no request is made and rotation changes nothing, even with `{ "on": true, … }` stored.
3. On a touch device with nothing stored, a day page loads with On in landscape, requesting a `"screen"` lock, and with Off in portrait, requesting nothing.
4. On a touch device, rotating to landscape checks On and requests a lock. Rotating to portrait checks Off and releases it. Each rotation removes `sous-keep-awake`.
5. On a touch device, tapping Off in landscape releases the lock and stores `{ "on": false, "orientation": "landscape" }`. A reload or another day page in landscape stays Off, and the next rotation into landscape turns it On. Tapping On in portrait works the same way the other round.
6. On a desktop, the choice is the stored `on`, or Off with nothing stored, in either orientation. Orientation changes do nothing. A tapped choice survives a reload.
7. While the choice is On, after the browser releases the lock and the page becomes visible again, a new lock is requested. While it's Off, visibility changes request nothing.
8. After a rejected request, the page shows no error and logs nothing to the console, and the next visibility change or click requests again. A click while a lock is held, or with Off, requests nothing. Requests never overlap, and a sentinel that arrives after the choice became Off is released.
9. With local storage throwing on access, or holding a malformed value, the page renders, the load rule applies as if nothing were stored, and taps and rotations still work until reload.
10. No request is sent to the server when the choice changes, the device rotates, or a lock is taken or released.
11. The Keep awake group matches the Theme group's type, colours, underline, hover and focus outline, in light and in dark. The groups share one line with the dot above the pinned breakpoint, and stack without the dot below it, at 320 px and above, with nothing overflowing.
12. The existing tests pass, updated where they count footer controls or radios.

## Verification

- Browser tests stub `navigator.wakeLock` with a script that runs before the page's own. The stub counts `request` calls, returns sentinels that the test can release (firing `release`), and can be set to reject or to resolve late. Pointer and orientation are stubbed by wrapping `matchMedia` for `(pointer: coarse)` and `(orientation: landscape)`, with a way to flip the orientation query and fire its `change` event. Visibility is simulated by overriding `document.visibilityState` and dispatching `visibilitychange`.
  - The group's presence on day and missing-plan pages and its absence on the other page kinds. With the stub removed and `on: true` stored: no group, no request, and a rotation changes nothing.
  - Touch device: the load state in each orientation with nothing stored, with a matching stored tap and with a mismatching one. Rotations each way: the checked radio, the request and release counts, and the removed key. A tap in each orientation: the stored object, the state after a reload, and the next rotation ending it.
  - Desktop: Off with nothing stored, the stored choice in both orientations, rotation ignored, and a tap that survives a reload.
  - Release, then becoming visible: a new request with On, none with Off.
  - A rejecting stub: no console error, a retry on the next visibility change and on the next click, and no request on a click while a lock is held or with Off. A late-resolving stub: one request for a load, a rotation, a visibility change and a click together, and a release when the choice becomes Off before the sentinel arrives.
  - Local storage replaced by a throwing stub, and then a malformed stored value: the page renders, the load rule applies, and taps and rotations work.
  - No network request on choosing, rotating, or lock changes.
  - Computed styles of the Keep awake options matching the Theme options, chosen and unchosen. Layout: one line with a visible dot at 1280 px and just above the breakpoint, two lines with the dot hidden just below it, and no horizontal overflow at 320 px.
  - Screenshots at 375 and 1280 px, light and dark, of a day page's footer.
- Manual check by the user, before Done, on the household's phone or tablet over HTTPS. Turned to landscape, Keep awake shows On and the screen stays on past the device's auto-lock time. Turned back to portrait, it shows Off and the screen sleeps normally. Tapping On in portrait keeps the screen on. On a laptop, it stays Off until tapped.
