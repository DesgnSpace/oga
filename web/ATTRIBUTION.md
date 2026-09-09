# Attribution — Beautiful UI

Components in `web/src/components/beautiful-ui/` are from [Beautiful UI](https://www.beautifului.dev/) — crafted primitives for AI-native interfaces.

- **Source:** vendored copy at `.malico/vendor/beautiful-ui/components/*.tsx` extracted from `beautifului.dev-home.html`
- **License:** MIT — as stated on the site
- **Modifications:** none to component source; six internal atoms (`Button`, `EntityChip`, `Shimmer`, `StreamText`, `ValuePill`, `GlideMenu`) were written locally because the library does not publish them, and a token stylesheet (`web/src/beautiful-ui.css`) was added to provide the CSS variables and utility mappings the components expect.
