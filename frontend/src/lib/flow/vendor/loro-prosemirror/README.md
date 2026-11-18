# SV Page binding patch

Based on installed loro-prosemirror 0.4.3 source, MIT license retained in LICENSE.

SV modifications are confined to Page root structure and lifecycle:

- `page-structure.ts`: read native `flow_editor_structure` preorder; map each Tree node's `block_cid` to the original physical `doc.children` Map. Root diffs operate on a logical view; new Maps append physically, removed Maps remain stored, and native Tree nodes manage parent/order/deletion.
- `lib.ts`: attach document context, derive PM indent from Tree depth, and route indexed root reads/writes through that view. Type conversion updates nodeName on the same Map.
- `cursor/common.ts`: root cursor offsets follow logical Tree order.
- `sync-plugin.ts`: attach context, refresh local explicit structure moves, invalidate cached root on indexed changes, unsubscribe when the view is destroyed.
- `index.ts`: expose the two bounded structure helpers used by the adapter.

An absent structure marker retains installed-library flat behavior. Only the server initializes an old document's index. Schema/HTML parsing, per-block native LoroText updates, IME tracking, and undo implementation remain the upstream code paths. The application supplies a shared document-lifetime UndoManager.
