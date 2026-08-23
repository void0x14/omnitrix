export default {
  // NOTE: FOR markdown, javascript and typescript, we use the opentui built-in parsers
  // Warn: when taking queries from the nvim-treesitter repo, make sure to include the query dependencies as well
  //       marked with for example `; inherits: ecma` at the top of the file. Just put the dependencies before the actual query.
  //       ALSO: Some queries use breaking changes in the nvim-treesitter repo, that are not compatible with the (web-)tree-sitter parser.
  parsers: [
    {
      filetype: "bash",
      // Vendored locally so the first parse of a bash block never blocks on a
      // network fetch (previously loaded from github releases on first paint).
      wasm: new URL("./bash/tree-sitter-bash.wasm", import.meta.url).href,
      queries: {
        highlights: [
          new URL("./bash/highlights.scm", import.meta.url).href,
        ],
      },
    },
    {
      filetype: "json",
      // Vendored locally so the first parse of a json block never blocks on a
      // network fetch (previously loaded from github releases on first paint).
      wasm: new URL("./json/tree-sitter-json.wasm", import.meta.url).href,
      queries: {
        highlights: [
          new URL("./json/highlights.scm", import.meta.url).href,
        ],
      },
    },
  ],
}
