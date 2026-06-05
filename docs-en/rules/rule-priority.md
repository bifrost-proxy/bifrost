> Language: **English** | [中文](../../docs/rules/rule-priority.md)

# Rule Priority and Execution Order

Bifrost rule execution follows two principles.

1. Routing rules are exclusive: the first matched routing rule wins.
2. Modification rules can merge: operations on the same part of a request or response are merged, and later rules override earlier values for the same field.

```txt
www.example.com host://server1.local
www.example.com host://server2.local
```

The request goes to `server1.local` because the first routing rule wins. Modification operations such as `reqHeaders` and `resHeaders` can combine, with later values replacing earlier values for identical keys.
