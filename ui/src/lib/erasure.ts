/**
 * What the screen offers once the shown Root's history was erased (#33): `refresh` while it is watched, its
 * history starting over; `manage`, the management page, once it is not, since nothing will capture it.
 * A plain module, so that the texts keyed by it need not import the reactive one that decides it.
 */
export type Erasure = 'refresh' | 'manage'
