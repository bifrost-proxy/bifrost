import type { Rule } from "antd/es/form";

export const skillNameRules: Rule[] = [
  { required: true },
  { pattern: /^[a-z][a-z0-9-]{0,63}$/, message: "Use kebab-case" },
];

export const skillDescriptionRules: Rule[] = [{ required: true, max: 1024 }];

export const skillSlashCommandRules: Rule[] = [
  { pattern: /^\/[a-z][a-z0-9-]{0,31}$/, message: "Use /kebab-case" },
];

export const requiredRule: Rule[] = [{ required: true }];
