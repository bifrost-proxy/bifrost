> Language: **English** | [中文](../../docs/rules/url-manipulation.md)

# URL Manipulation Rules

URL manipulation rules add, replace, or remove URL components.

```txt
example.com urlParams://debug=true
example.com urlParams://(version: 2)
example.com urlReplace://v1=v2
example.com urlReplace://(/v\d+/=v2)
```

Template variables such as `${now}` and `${randomUUID}` expand inside a `urlParams://` value **without** backticks. Backticks are only needed to protect a value that contains spaces (which the rule-line tokenizer would otherwise split), not to enable template expansion. Embedded values are recommended for multi-parameter configuration, and referencing an embedded value (`urlParams://{my-params}`) also needs no backticks.
