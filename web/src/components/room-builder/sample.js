// A small hand-built hamlet, so the room builder is a live demo when it isn't
// being served by an engine (standalone dev, or before login). Shape matches
// the list_world_slice payload: rooms carry area + tags, and one exit leaves
// the slice to a boundary stub the canvas can offer to expand.

export const sampleAreas = [
  { area: 'village', count: 7 },
  { area: 'iron_hills', count: 12 },
  { area: 'chapel_undercroft', count: 4 },
];

export const sampleWorld = {
  rooms: [
    { ref: '#12', key: 'green', title: 'The Village Green', area: 'village', tags: ['spawn:town'],
      description: 'A commons of cropped grass rings the old market cross. Lanes run off toward the tavern and the ring of the smithy.' },
    { ref: '#13', key: 'stag', title: 'The Last Stag', area: 'village', tags: ['shop:tavern'],
      description: "Low beams, a peat fire, and the smell of spilled ale. A stuffed stag's head watches the door." },
    { ref: '#14', key: 'smithy', title: "Halden's Smithy", area: 'village', tags: ['shop:smith'],
      description: 'Heat rolls off the forge in waves. Tongs and half-finished blades hang from a soot-black wall.' },
    { ref: '#15', key: 'chapel', title: 'Chapel of the Stag', area: 'village', tags: ['quest:vigil'],
      description: 'Cool stone and guttering votive candles. A carved stag stands antler-proud above the altar.' },
    { ref: '#16', key: 'gate_s', title: 'The South Gate', area: 'village', tags: [],
      description: 'Two weathered posts and a swinging lantern mark the edge of the village.' },
    { ref: '#17', key: 'well', title: 'The Old Well', area: 'village', tags: [],
      description: 'A mossed wellhead with a frayed rope. Coins wink in the dark water below.' },
    { ref: '#18', key: 'undercroft', title: 'Chapel Undercroft', area: 'village', tags: ['dungeon:entrance'],
      description: 'Steps descend into cold dark beneath the chapel. The air smells of old stone and older secrets.' },
  ],
  exits: [
    { ref: '#40', from: '#12', dir: 'e', to: '#13' }, { ref: '#41', from: '#13', dir: 'w', to: '#12' },
    { ref: '#42', from: '#12', dir: 'w', to: '#14' }, { ref: '#43', from: '#14', dir: 'e', to: '#12' },
    { ref: '#44', from: '#12', dir: 'n', to: '#15' }, { ref: '#45', from: '#15', dir: 's', to: '#12' },
    { ref: '#46', from: '#12', dir: 's', to: '#16' }, { ref: '#47', from: '#16', dir: 'n', to: '#12' },
    { ref: '#48', from: '#12', dir: 'ne', to: '#17' }, { ref: '#49', from: '#17', dir: 'sw', to: '#12' },
    { ref: '#50', from: '#15', dir: 'down', to: '#18' }, { ref: '#51', from: '#18', dir: 'up', to: '#15' },
    // leaves the slice: the road south into the iron hills (a boundary stub)
    { ref: '#52', from: '#16', dir: 's', to: '#90' },
  ],
  boundary: [
    { ref: '#90', key: 'gorse_road', title: 'Gorse Road' },
  ],
  truncated: false,
};
