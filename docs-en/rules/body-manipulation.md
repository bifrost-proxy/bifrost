> Language: **English** | [中文](../../docs/rules/body-manipulation.md)

# Body Manipulation Rules

Body manipulation rules modify request or response bodies.

```txt
example.com reqBody://({"debug":true})
example.com resBody://(mock response)
example.com file://./fixtures/response.json
example.com tpl://./fixtures/template.html
```

`reqReplace` and `resReplace` keep supporting the legacy `old=new&foo=bar` form. They also accept a referenced strict JSON object whose keys are search strings and whose values are replacements:

````txt
```replace
{
  ".doupay.com\"": ".nodoupay.com\"",
  "\"inf.baohuaxia.com\"": "\"inf.nobaohuaxia.com\""
}
```

*/get_domains/v5 resReplace://{replace}
````

The same Values form works with `reqReplace://{replaceMap}`. Header Values use `reqHeaders://{headers}`. To replace a substring inside a request header, use the separate `headerReplace://{headerRules}` protocol whose referenced Value keeps the `req.header-name:old=new` syntax; it is not a body-replace JSON map.

JSON string escaping is decoded before matching. A key written as a regex literal, such as `"/foo/g"`, keeps the existing regex replacement behavior. This compatibility form requires valid JSON with double-quoted keys; single-quoted and YAML-like objects are not accepted.

For large, binary, or streaming bodies, prefer file-based mocks or scripts. Keep body rewrites scoped to precise patterns to avoid accidental broad modification.
