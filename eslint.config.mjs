// Flat ESLint config so DeepSource's ESLint-based JS analyzer parses every `.js` as an ES module.
// ESLint v9 reads only flat config; the sibling .eslintrc.json covers v8. No rules -- oxlint handles repo linting.
export default [
  {
    files: ["**/*.js"],
    languageOptions: {
      ecmaVersion: "latest",
      sourceType: "module",
    },
  },
];
