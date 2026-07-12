import type { ThemeColors, ThemeElevation, ThemeModeTokens, ThemeMotion, ThemePack, ThemeShape, ThemeTypography } from "./types";

type FontStyle = "geist" | "system" | "serif" | "mono";
type ShapeStyle = "sharp" | "compact" | "rounded" | "plush";
type RawPalette = readonly [
  canvas: string, surface: string, surface2: string, ink: string, inkSoft: string,
  muted: string, faint: string, line: string, lineStrong: string, accent: string,
  accentTint: string, good: string, bad: string, hoverSoft: string,
];
type RawSpec = readonly [
  id: string, name: string, description: string, font: FontStyle, shape: ShapeStyle,
  light: RawPalette, dark: RawPalette,
];

const FONT_STACKS: Record<FontStyle, ThemeTypography> = {
  geist: { fontUi: '"Geist Variable", ui-sans-serif, system-ui, sans-serif', fontDisplay: '"Geist Variable", ui-sans-serif, system-ui, sans-serif', fontMono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', baseSize: "15px", scale: 1 },
  system: { fontUi: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif', fontDisplay: '-apple-system, BlinkMacSystemFont, "SF Pro Text", "Helvetica Neue", sans-serif', fontMono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', baseSize: "15px", scale: 1 },
  serif: { fontUi: 'Iowan Old Style, "Palatino Linotype", Palatino, Georgia, serif', fontDisplay: 'Iowan Old Style, "Palatino Linotype", Palatino, Georgia, serif', fontMono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', baseSize: "15px", scale: 1 },
  mono: { fontUi: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', fontDisplay: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', fontMono: '"SFMono-Regular", Consolas, "Liberation Mono", monospace', baseSize: "15px", scale: 1 },
};

const SHAPES: Record<ShapeStyle, ThemeShape> = {
  sharp: { radiusSm: "1px", radiusMd: "2px", radiusLg: "3px", radiusXl: "4px", borderWidth: "1px" },
  compact: { radiusSm: "5px", radiusMd: "7px", radiusLg: "10px", radiusXl: "14px", borderWidth: "1px" },
  rounded: { radiusSm: "10px", radiusMd: "14px", radiusLg: "18px", radiusXl: "24px", borderWidth: "1px" },
  plush: { radiusSm: "14px", radiusMd: "18px", radiusLg: "24px", radiusXl: "32px", borderWidth: "1px" },
};

const MOTION: Record<ShapeStyle, ThemeMotion> = {
  sharp: { durationFast: "80ms", durationNormal: "140ms", durationSlow: "220ms", easing: "linear" },
  compact: { durationFast: "110ms", durationNormal: "180ms", durationSlow: "280ms", easing: "cubic-bezier(0.2, 0.8, 0.2, 1)" },
  rounded: { durationFast: "140ms", durationNormal: "220ms", durationSlow: "360ms", easing: "cubic-bezier(0.22, 1, 0.36, 1)" },
  plush: { durationFast: "180ms", durationNormal: "280ms", durationSlow: "500ms", easing: "cubic-bezier(0.16, 1, 0.3, 1)" },
};

function elevation(shape: ShapeStyle, dark: boolean): ThemeElevation {
  if (shape === "sharp") return { shadowSm: "none", shadowMd: `0 0 0 1px rgba(0, 0, 0, ${dark ? "0.45" : "0.12"})`, shadowLg: `0 0 0 1px rgba(0, 0, 0, ${dark ? "0.7" : "0.2"})`, blur: "0px" };
  if (shape === "compact") return { shadowSm: `0 1px 2px rgba(0, 0, 0, ${dark ? "0.28" : "0.05"})`, shadowMd: `0 8px 22px rgba(0, 0, 0, ${dark ? "0.42" : "0.11"})`, shadowLg: `0 20px 48px rgba(0, 0, 0, ${dark ? "0.58" : "0.18"})`, blur: "6px" };
  if (shape === "rounded") return { shadowSm: `0 2px 6px rgba(0, 0, 0, ${dark ? "0.28" : "0.06"})`, shadowMd: `0 12px 32px rgba(0, 0, 0, ${dark ? "0.44" : "0.12"})`, shadowLg: `0 28px 68px rgba(0, 0, 0, ${dark ? "0.6" : "0.2"})`, blur: "12px" };
  return { shadowSm: `0 3px 10px rgba(0, 0, 0, ${dark ? "0.28" : "0.07"})`, shadowMd: `0 18px 44px rgba(0, 0, 0, ${dark ? "0.46" : "0.14"})`, shadowLg: `0 36px 88px rgba(0, 0, 0, ${dark ? "0.62" : "0.22"})`, blur: "22px" };
}

function channels(hex: string): [number, number, number] {
  const value = hex.replace("#", "").toLowerCase();
  return [0, 2, 4].map((index) => Number.parseInt(value.slice(index, index + 2), 16)) as [number, number, number];
}

function hex(values: readonly number[]): string {
  return `#${values.map((value) => Math.round(Math.max(0, Math.min(255, value))).toString(16).padStart(2, "0")).join("")}`;
}

function mix(first: string, second: string, amount: number): string {
  const a = channels(first);
  const b = channels(second);
  return hex(a.map((value, index) => value + (b[index] - value) * amount));
}

function luminance(color: string): number {
  return channels(color)
    .map((value) => value / 255)
    .map((value) => value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4)
    .reduce((sum, value, index) => sum + value * [0.2126, 0.7152, 0.0722][index], 0);
}

function contrast(first: string, second: string): number {
  const a = luminance(first);
  const b = luminance(second);
  return (Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05);
}

function contrastText(background: string): string {
  return contrast("#000000", background) >= contrast("#ffffff", background) ? "#000000" : "#ffffff";
}

function readable(color: string, background: string): string {
  if (contrast(color, background) >= 4.5) return color.toLowerCase();
  const target = contrastText(background);
  for (let step = 1; step <= 20; step += 1) {
    const candidate = mix(color, target, step / 20);
    if (contrast(candidate, background) >= 4.5) return candidate;
  }
  return target;
}

function palette(raw: RawPalette): ThemeColors {
  const [canvas, surface, surface2, rawInk, rawInkSoft, rawMuted, rawFaint, line, lineStrong, accent, accentTint, good, bad, hoverSoft] = raw.map((color) => color.toLowerCase()) as unknown as RawPalette;
  const ink = readable(rawInk, surface);
  const inkSoft = readable(rawInkSoft, surface);
  const muted = readable(rawMuted, surface);
  const faint = readable(rawFaint, surface);
  const errorBg = mix(surface, bad, 0.1);
  return {
    canvas, surface, surface2, ink, inkSoft, muted, faint, line, lineStrong, accent, accentTint, good, bad,
    onInk: contrastText(ink), inkHover: mix(ink, surface, 0.12), hoverSoft,
    scrollThumb: mix(surface, ink, 0.18), scrollThumbHover: mix(surface, ink, 0.3),
    errorBg, errorBorder: mix(surface, bad, 0.3), errorInk: readable(bad, errorBg),
  };
}

function themeMode(raw: RawPalette, font: FontStyle, shape: ShapeStyle, dark: boolean): ThemeModeTokens {
  const colors = palette(raw);
  return {
    colors,
    typography: FONT_STACKS[font],
    shape: SHAPES[shape],
    elevation: elevation(shape, dark),
    motion: MOTION[shape],
    charts: [colors.accent, colors.good, colors.bad, mix(colors.accent, colors.good, 0.48), mix(colors.accent, colors.bad, 0.48), colors.inkSoft, colors.muted, colors.lineStrong],
  };
}

function makeTheme([id, name, description, font, shape, light, dark]: RawSpec): ThemePack {
  return { schemaVersion: 1, id, name, description, source: { kind: "builtin", label: "Included with Noted" }, light: themeMode(light, font, shape, false), dark: themeMode(dark, font, shape, true) };
}

const SPECS = [
  ["porcelain-blue", "Porcelain Blue", "An airy workspace with porcelain surfaces and restrained blue signals.", "geist", "rounded", ["#f4f7fa","#ffffff","#eaf0f6","#17202b","#303d4c","#4d5b6b","#657183","#d4dde7","#abb8c7","#2864c7","#ddeaff","#18794e","#b42335","#e6edf5"], ["#0d1218","#141b23","#1c2631","#f2f6fa","#d4dce5","#b5c0cc","#9aa7b5","#2c3947","#46576a","#79a9ff","#173564","#61d6a1","#ff8793","#202c38"]],
  ["saffron-ledger", "Saffron Ledger", "A parchment ledger grounded by confident saffron accents.", "serif", "compact", ["#f7f2e8","#fffcf6","#eee5d5","#241e16","#40372b","#5e5344","#746653","#ded1bc","#bbaa8f","#a85f00","#f7e2b7","#367249","#a83232","#f1e8d9"], ["#17130d","#201a12","#2b2318","#fff8e9","#e7dcc7","#c6b89f","#aa9b83","#403424","#66543a","#f2b84b","#49320e","#7bd39a","#f28a82","#30271b"]],
  ["alpine-slate", "Alpine Slate", "Mineral grays and a glacial accent create cool high-altitude precision.", "system", "sharp", ["#f1f4f4","#fbfcfc","#e4e9e9","#182121","#334040","#4f5e5e","#647171","#cdd6d6","#a4b2b2","#087e8b","#d0f0f1","#237a4b","#b1323e","#e7ecec"], ["#0e1314","#151c1d","#1e292a","#f1f7f7","#d2dede","#b1c1c1","#97a9a9","#304041","#4a5e60","#55d3db","#123d41","#69d497","#fa8992","#223031"]],
  ["coral-studio", "Coral Studio", "A bright creative studio with soft curves and lively coral direction.", "geist", "plush", ["#fff5f2","#fffcfb","#fbe7e1","#2a1917","#49312e","#654d49","#79615d","#e9cbc3","#cda59b","#c83f31","#ffdad3","#24754b","#a9273a","#fcebe7"], ["#190f0e","#231614","#31201d","#fff5f2","#ead6d1","#c9aaa3","#ae918b","#49302c","#6c4841","#ff8b7d","#55231d","#73d29c","#ff8a9a","#37231f"]],
  ["sage-workshop", "Sage Workshop", "A calm utilitarian workspace shaped by sage, canvas, and charcoal.", "system", "rounded", ["#f3f5ee","#fcfdf9","#e5e9dc","#20251d","#394135","#566151","#6a7465","#d0d7c7","#aab5a0","#52733f","#dde9d2","#277044","#a63a38","#e9ede3"], ["#11140f","#191d16","#242a20","#f2f6ec","#d7dfce","#b6c1ac","#9da993","#374031","#536049","#9bc47e","#2c4221","#72d294","#f08b86","#293025"]],
  ["cobalt-grid", "Cobalt Grid", "A dense information system with sharp geometry and electric cobalt focus.", "mono", "sharp", ["#f3f5fa","#ffffff","#e8ecf5","#151a26","#30384a","#4d576c","#626d83","#d0d6e3","#a4aec1","#174bc4","#dce6ff","#137347","#b4263b","#e9edf6"], ["#090d17","#101624","#192239","#f3f6ff","#d7deef","#b4bed5","#99a6c1","#293552","#425275","#739aff","#1b3271","#5fd49a","#ff8298","#1d2840"]],
  ["aubergine-press", "Aubergine Press", "A literary interface with restrained aubergine and modern press authority.", "serif", "compact", ["#f7f2f5","#fffcfe","#ece2e9","#281c25","#453440","#614f5b","#766370","#ddcfd8","#b9a4b2","#763c72","#f0d9ed","#34734e","#aa3143","#f0e7ed"], ["#160f14","#20171e","#2c202a","#fff5fc","#e7d5e2","#c7acbf","#ac92a4","#443340","#664d60","#d493ce","#482344","#78d39d","#f18a99","#32252f"]],
  ["nordic-fog", "Nordic Fog", "Misty neutrals and understated arctic blue create quiet spaciousness.", "geist", "rounded", ["#f2f5f6","#fcfdfd","#e5ebed","#1a2225","#354146","#526066","#67747a","#cfd9dc","#a7b5ba","#3a6f86","#d9eaf1","#28734c","#ac3540","#e8edef"], ["#101416","#181e21","#222c30","#f2f7f8","#d5e0e3","#b4c2c7","#9aaab0","#344247","#506168","#84bdd5","#213f4b","#70d19b","#f08b93","#273338"]],
  ["sepia-archive", "Sepia Archive", "A scholarly archive of aged paper warmth and disciplined brown type.", "serif", "sharp", ["#f3ebdd","#fcf7ee","#e8dcc8","#2a2117","#493b2c","#655644","#796956","#d6c5aa","#b09b7b","#8a4b25","#efd7be","#3b7045","#9f3430","#ece1cf"], ["#17120c","#211a12","#2e2419","#fff7e8","#e8dac3","#c8b79c","#ad9b80","#453725","#68543a","#d99461","#4c2c18","#81cf94","#ee8a80","#33281c"]],
  ["mint-circuit", "Mint Circuit", "A technical workspace energized by mint signals and graphite structure.", "mono", "compact", ["#eff7f4","#fbfefd","#ddeee8","#14231f","#2e443d","#496159","#60766e","#c8ddd6","#9ebab0","#087a5b","#cff0e4","#187547","#af3341","#e2f0eb"], ["#091411","#101d19","#172a24","#edfff8","#cde9df","#aad0c2","#8fb5a8","#28453b","#3e6859","#55dcaf","#164c3b","#61d18e","#f18491","#1b312a"]],
  ["vermilion-desk", "Vermilion Desk", "Ink-black structure with sparing vermilion emphasis.", "geist", "sharp", ["#f6f4f1","#fffdfc","#ede8e3","#211d1a","#3e3833","#5b534d","#706760","#d9d1ca","#b4aaa1","#b73722","#f6d8d0","#267147","#a72532","#eee9e5"], ["#13100e","#1c1815","#28221e","#fff8f3","#e4d8d0","#c2b2a8","#a7958a","#3d342e","#5d4f46","#ff806a","#51251c","#70d294","#f4838b","#2d2722"]],
  ["iris-signal", "Iris Signal", "Neutral foundations with vivid iris interaction states.", "system", "plush", ["#f6f4fa","#fefdff","#eae6f2","#211c2b","#3e364d","#5a506b","#6e647d","#d8d1e3","#b1a6c1","#6842c2","#e8deff","#2e744e","#ad3045","#eeeaf5"], ["#110e18","#191522","#241e31","#faf6ff","#ded5ec","#bdb0d0","#a294b6","#392f4b","#57476f","#ac8cff","#38266c","#73d39b","#f28a9e","#2a2338"]],
  ["ocean-ledger", "Ocean Ledger", "Financial clarity balanced with a calm maritime atmosphere.", "geist", "compact", ["#eff6f7","#fcfefe","#ddecef","#142327","#2e4248","#496066","#60757b","#c7dcdf","#9db9be","#116f82","#d1edf2","#247249","#ae3440","#e2eff1"], ["#091316","#101d21","#182a30","#effcff","#cfe7ec","#adcdd4","#91b3bb","#28454d","#3f6771","#5cc9de","#174653","#68d194","#f08791","#1c3137"]],
  ["rosewood-journal", "Rosewood Journal", "An intimate cream-paper writing environment with rosewood depth.", "serif", "rounded", ["#f6efed","#fff9f7","#ebddd8","#2b1d1d","#493333","#654d4c","#79615f","#ddc9c3","#b99f97","#884548","#f2d8d6","#397149","#a52e3c","#efe3df"], ["#160e0e","#211616","#2d1f1f","#fff5f2","#e6d3ce","#c5aaa4","#aa8e88","#44302f","#674947","#d9878b","#4b2527","#7ad099","#f18894","#332424"]],

  ["alpine-morning", "Alpine Morning", "Cool mountain air, pale stone, and evergreen accents.", "geist", "rounded", ["#f3f7f4","#ffffff","#e8efeb","#17251e","#34483d","#52645a","#637169","#cbd8d0","#9fb1a6","#287557","#d8eee4","#247047","#b13d3d","#e6eee9"], ["#0d1511","#131e18","#1a2921","#edf6f0","#cfddd4","#b6c5bc","#a2b2a8","#2d4035","#496052","#72d3aa","#183b2d","#63cb91","#f08080","#203128"]],
  ["redwood-fog", "Redwood Fog", "Deep forest greens and muted fog beneath old redwoods.", "serif", "rounded", ["#f2f4f0","#fbfcfa","#e5e9e2","#20291f","#3c493a","#586456","#687267","#cdd4ca","#a6b0a2","#506f58","#dde8df","#39734b","#a9443e","#e7ebe5"], ["#101310","#181c18","#222822","#f0f3ed","#d2d8ce","#b8c0b4","#a4ada1","#343b33","#535e51","#8fba98","#26382a","#78c48e","#e98278","#272d27"]],
  ["desert-clay", "Desert Clay", "Sun-warmed clay, dry sand, and sparse desert greens.", "geist", "plush", ["#f8f1e8","#fffaf3","#eee0d1","#33251e","#503d32","#675447","#766257","#ddc9b7","#bca28d","#a64f32","#f2d7c8","#577044","#ad3838","#f1e4d7"], ["#18110d","#211813","#30231b","#faeee4","#dfcfc2","#c6b3a5","#b39f91","#49362b","#6a5040","#e58762","#4a271c","#98bd75","#ee7d72","#35271f"]],
  ["coastal-glass", "Coastal Glass", "Sea glass, salt haze, and subdued ocean blues.", "system", "plush", ["#f1f7f7","#fbfefe","#e1eeee","#173033","#334c4f","#50666a","#617579","#c6dada","#99b9ba","#217a83","#d3ecee","#317556","#ac3f47","#e2efef"], ["#0b1517","#111e21","#192b2f","#ecf8f8","#cce1e2","#b1c9cb","#9fb7ba","#294247","#43636a","#66cbd3","#153a3e","#6bc49a","#ef7c84","#203338"]],
  ["mossy-cabin", "Mossy Cabin", "Weathered timber, shaded moss, and warm lamplight.", "serif", "compact", ["#f3f0e7","#fbf8ef","#e7e1d3","#29291e","#454536","#5f5f4d","#6d6c59","#d2cbb9","#aaa28c","#5c713b","#e1e7cf","#55723e","#a8433c","#e9e4d8"], ["#12120d","#1b1b14","#28271d","#f3f0e4","#d9d5c6","#bfbbab","#aaa696","#3b3a2d","#5c5945","#a6c06f","#314021","#8fc171","#e47f74","#2d2c21"]],
  ["winter-linen", "Winter Linen", "Soft woven whites and blue-gray winter shadows.", "serif", "rounded", ["#f4f5f3","#fdfdfb","#e8ebe9","#252b2d","#404a4d","#596569","#687377","#d0d6d5","#a6b0b0","#557887","#dce8ec","#477257","#a74549","#e9edec"], ["#111416","#191d1f","#242a2d","#f1f3f1","#d5d9d8","#bbc2c1","#a7afae","#363d40","#555f63","#91bccd","#243b45","#80bd95","#e48286","#2a3033"]],
  ["autumn-orchard", "Autumn Orchard", "Apple reds, fallen leaves, and muted olive notes.", "geist", "rounded", ["#f8f2e9","#fffaf2","#eee2d3","#33271f","#503f34","#68574a","#76665a","#ddcdbb","#baa48e","#a34436","#f2d8cf","#5e733e","#ae3838","#f1e6d9"], ["#18120e","#211914","#30241c","#f9eee3","#dfcfc0","#c5b3a3","#b19e8f","#49382d","#6a5342","#e6816f","#4a251e","#a4bf70","#ef7a74","#35291f"]],
  ["midnight-aurora", "Midnight Aurora", "Polar night blues with luminous green-violet energy.", "geist", "sharp", ["#f1f4f8","#fbfcff","#e3e9f1","#1d2635","#38465a","#536176","#637086","#cbd4e0","#9fadc0","#4c628f","#dce4f5","#34745e","#a83f52","#e6ebf3"], ["#090d17","#101625","#192137","#eef3ff","#d0daed","#b6c2da","#a2aec7","#2b3650","#465573","#87e3c2","#173d3b","#78d0a7","#f07d95","#202a42"]],
  ["lavender-field", "Lavender Field", "Dusty lavender, herb green, and warm chalk tones.", "serif", "plush", ["#f6f2f6","#fefbfe","#eae2eb","#302832","#4b404e","#645768","#726575","#d8cbd9","#b39fb5","#7b5684","#eadbef","#527250","#a63f52","#eee5ef"], ["#151116","#1e1820","#2c2330","#f6edf6","#ddcedf","#c3b1c6","#af9db2","#423545","#625066","#c597d0","#3d2942","#91bd8d","#eb7f92","#322936"]],
  ["volcanic-stone", "Volcanic Stone", "Basalt blacks, mineral grays, and ember orange.", "mono", "sharp", ["#f1f1ef","#fafaf8","#e4e4e1","#242423","#41413f","#5c5c59","#6b6b67","#cececa","#a4a49e","#a64c2b","#f1d9cf","#47704f","#ad3939","#e8e8e4"], ["#0e0e0d","#171716","#232321","#f1f1ed","#d4d4ce","#bbbbb3","#a7a79f","#363633","#565650","#ed865e","#48261b","#80bd8a","#ee7a76","#292927"]],
  ["arctic-blue", "Arctic Blue", "Glacial blue, packed snow, and clear polar shadows.", "system", "compact", ["#f0f6f9","#fbfdff","#e1ecf2","#192c38","#354c59","#526774","#627783","#c6d8e1","#98b6c5","#286f93","#d4eaf5","#34745d","#aa3e4b","#e2eef3"], ["#0a1217","#101c23","#192a34","#edf7fb","#cee2eb","#b4cbd6","#a0b8c3","#29414e","#436272","#70c6ec","#17384a","#70c69e","#ed7c87","#20333e"]],
  ["sakura-rain", "Sakura Rain", "Rain-washed stone and restrained blossom pink.", "serif", "rounded", ["#f7f3f4","#fffafa","#ece3e5","#33282b","#504044","#69585c","#77666a","#dccdd0","#b9a3a8","#9b5266","#f1dce2","#547255","#aa3b4a","#f0e6e8"], ["#161112","#201819","#2e2325","#f8edef","#dfcfd2","#c5b2b6","#b19fa3","#443538","#655055","#df91a6","#452832","#91bd91","#ed7b88","#34282a"]],
  ["canyon-dusk", "Canyon Dusk", "Layered sandstone, violet distance, and evening copper.", "geist", "compact", ["#f7f0e9","#fff9f3","#ecded3","#34261f","#523d33","#6a5549","#786358","#dbc7b9","#b99e8b","#98533d","#efd8cd","#5b7247","#aa3c3c","#f0e2d7"], ["#17100d","#211713","#30221c","#f8ece4","#dfcdc2","#c5b1a5","#b19d91","#48342b","#695044","#e28a6b","#47281d","#9abd7a","#ed7a73","#35261f"]],
  ["terracotta-studio", "Terracotta Studio", "Plaster walls, pottery, and inked workshop labels.", "mono", "rounded", ["#f6efe7","#fef9f2","#eadfd2","#32261f","#4f3d33","#665448","#756258","#d9c8b7","#b69e89","#a45237","#f0d8cc","#5a7145","#ab3c3c","#eee3d7"], ["#17110d","#201813","#2e231c","#f7eee5","#ddcfc3","#c3b4a7","#afa095","#44362c","#645145","#e68b69","#49291e","#9abd78","#ed7b73","#332920"]],

  ["phosphor-grove", "Phosphor Grove", "A green-screen workspace softened with botanical undertones.", "mono", "compact", ["#f1f5ef","#ffffff","#e8eee5","#142017","#2f4034","#46594b","#60676f","#d5ded3","#afc1b1","#16843b","#d9f3df","#16843b","#b9363e","#edf3eb"], ["#090d09","#101410","#172019","#e9f5eb","#c4d5c7","#a4b5a7","#89988b","#28362b","#405545","#5af07c","#163a20","#5af07c","#ff6b74","#19251c"]],
  ["amber-relay", "Amber Relay", "Warm terminal amber and near-black communications panels.", "mono", "sharp", ["#f8f3e8","#fffdf8","#f0e8d8","#241b0d","#413621","#574c38","#625f57","#e0d3ba","#bfa77c","#a95700","#ffe5b7","#36733a","#b43a32","#f5eddd"], ["#0f0c08","#17130d","#211b12","#fff2d0","#dac9a6","#bdae91","#a79a83","#3b3020","#5d4a2c","#ffb23f","#422b0c","#79d486","#ff7569","#272015"]],
  ["lunar-console", "Lunar Console", "Cool navigation blues and disciplined orbital geometry.", "geist", "compact", ["#f3f6fb","#fbfcff","#e9eef6","#121b28","#303c4c","#465366","#5e6675","#d5deea","#a9b9cd","#2467d1","#dce9ff","#247747","#bc3545","#edf2f9"], ["#090d13","#10151d","#17202c","#edf4ff","#c9d5e5","#aab7c9","#8f9aae","#283446","#41536d","#69a4ff","#18365f","#63d597","#ff7181","#1a2533"]],
  ["violet-drift", "Violet Drift", "Deep-space violet and ion blue with restrained glow.", "geist", "rounded", ["#f6f2fa","#fcfaff","#eee7f5","#21172b","#403449","#584d62","#655e70","#ded4e8","#b9a6cc","#7046c8","#e9ddff","#28764d","#bd365c","#f2ecf8"], ["#0d0911","#15111b","#201829","#f6edff","#d8c9e4","#b7a9c3","#9b90a7","#34283f","#514064","#b48cff","#342052","#68d69e","#ff7198","#261d30"]],
  ["oxide-deck", "Oxide Deck", "Burnished rust and industrial machine-room neutrals.", "system", "sharp", ["#f7f1ec","#fffaf5","#ece2d9","#251a14","#44372f","#5d5048","#675f58","#dfd0c4","#bca28f","#a84420","#f9ddce","#39713d","#ad3030","#f2e9e1"], ["#100c09","#19130f","#241b15","#f8eee7","#d9c8bc","#b9a99d","#a4978b","#3b2d25","#5c4435","#f47b4d","#482113","#7bd185","#ff746b","#2a2019"]],
  ["pixel-picnic", "Pixel Picnic", "Primary-color sparks and tidy pixel-era structure.", "mono", "compact", ["#f5f2e9","#fffdf7","#eee9dc","#1c1c18","#393a34","#51534a","#625f55","#ddd7c8","#b9b09c","#e0442e","#ffe0d7","#2f793f","#bd3340","#f3efe5"], ["#0b0d10","#121519","#1b2026","#f5f5e9","#d2d4c9","#afb4b5","#9299a3","#303740","#4b5662","#ff6650","#49201b","#66d67d","#ff7180","#20262d"]],
  ["bubble-schema", "Bubble Schema", "Berry accents and buoyant surfaces for friendly structured information.", "geist", "plush", ["#f8f1f6","#fffafd","#f0e4ec","#281923","#463741","#5d4f59","#665e66","#e4d2df","#c2a5b8","#b43175","#f8d9e9","#2d774c","#ba3544","#f5eaf1"], ["#100b10","#181218","#241b23","#faeef7","#ddcbd7","#bcaeba","#a195a1","#3a2b38","#5c4256","#ff75b6","#4b1d35","#6bd49a","#ff727f","#2a2029"]],
  ["mono-ledger", "Mono Ledger", "Pure grayscale hierarchy and bookkeeping precision.", "serif", "sharp", ["#f4f4f2","#ffffff","#ececea","#181818","#363636","#505050","#626262","#d8d8d5","#adada9","#292929","#e5e5e3","#356a44","#9e3838","#f0f0ee"], ["#0d0d0d","#151515","#202020","#f2f2ef","#d1d1cd","#b0b0ac","#969696","#333333","#505050","#f0f0ed","#353535","#75c68d","#ef7777","#252525"]],
  ["graphite-signal", "Graphite Signal", "Dense charcoal and crisp signal accents for engineering work.", "mono", "compact", ["#f1f2f2","#fbfbfb","#e7e9e9","#17191a","#343738","#4d5152","#606060","#d2d6d6","#a6adae","#3b555c","#dbe6e8","#347047","#a93c42","#edf0f0"], ["#0b0c0d","#131415","#1d1f21","#f0f2f2","#cdd0d1","#acb0b2","#929496","#303335","#4c5154","#a9d4dd","#263b40","#76ca8f","#ef747b","#232527"]],
  ["polar-module", "Polar Module", "Frosted blue-gray surfaces with precise cobalt cues.", "system", "rounded", ["#f2f7fa","#fbfdff","#e6eef3","#14202a","#33414c","#4b5965","#5d6670","#d2dfe7","#a5bbc9","#176ca5","#d8edfa","#27754e","#b63a48","#eaf2f6"], ["#090f13","#11171c","#19232a","#edf7fc","#cad9e1","#aabac3","#8f9ca5","#2b3942","#445a67","#63bdf2","#183b50","#68d09c","#ff7482","#1e2a31"]],
  ["reactor-lime", "Reactor Lime", "Acid-lime instrumentation for a quiet scientific dashboard.", "geist", "sharp", ["#f3f7ef","#fbfff7","#e8efdf","#172013","#35422f","#4d5b46","#5f6958","#d5e0cc","#aabd9d","#4d7900","#e2f4bd","#32713a","#b53b3b","#edf5e7"], ["#0a0e08","#12180f","#1b2517","#effae9","#ceddc7","#afbea8","#92a18b","#2e3c27","#485f3c","#b3f25a","#304718","#7ddd87","#ff7772","#202c1b"]],
  ["mars-archive", "Mars Archive", "Terracotta and parchment records from a distant expedition.", "serif", "rounded", ["#f7eee9","#fffaf7","#ede0da","#281914","#47362f","#604e47","#6a5f5a","#e0cec5","#bda194","#a54532","#f6dcd3","#3d7045","#a93239","#f2e7e1"], ["#100a09","#1a1210","#251a17","#faeee9","#dccac3","#bda9a1","#a7968e","#3d2c27","#60453c","#ed8068","#49231d","#7bc889","#ff777d","#2b1f1b"]],
  ["cobalt-workshop", "Cobalt Workshop", "Toolbox blue and sturdy surfaces for building and debugging.", "system", "compact", ["#f2f5fa","#fbfcff","#e7ebf2","#151c27","#333d4b","#4a5565","#5d6672","#d2dae6","#a5b4c9","#245bb8","#dbe6fc","#2e7449","#b83846","#ebeff6"], ["#090d13","#10151c","#18212c","#eef3fb","#cbd5e3","#aab5c5","#8d99aa","#293546","#40536c","#72a5ff","#1b365e","#6bd297","#ff7482","#1c2733"]],
  ["solar-punch", "Solar Punch", "Sunlit yellow and electric coral with exuberant energy.", "geist", "plush", ["#f8f3df","#fffdf4","#f1e8c9","#231d0f","#423923","#594f38","#675f50","#e3d7b5","#bfad77","#c44731","#ffe0b8","#39733e","#b33142","#f5edda"], ["#100e08","#18150d","#242014","#fff5d4","#ddcfaa","#bcb08f","#9e9681","#3b3420","#5e502c","#ff7b62","#50251b","#7bd282","#ff7181","#292418"]],
] as const satisfies readonly RawSpec[];

export const ADDITIONAL_THEMES: readonly ThemePack[] = SPECS.map(makeTheme);
export const ADDITIONAL_THEME_IDS: readonly string[] = SPECS.map(([id]) => id);
