const path = require("path");

class VariableResolver {
  #variables = new Map();
  #builtins = {};

  constructor(initialVars = {}) {
    this.#initBuiltins();
    Object.entries(initialVars).forEach(([key, value]) => {
      this.set(key, this.resolve(value));
    });
  }

  #initBuiltins() {
    const now = new Date();
    this.#builtins = {
      timestamp: Date.now().toString(),
      date: now.toISOString().split("T")[0],
      datetime: now.toISOString().replace(/[:.]/g, "-"),
      time: now.toTimeString().split(" ")[0].replace(/:/g, "-"),
      year: now.getFullYear().toString(),
      month: (now.getMonth() + 1).toString().padStart(2, "0"),
      day: now.getDate().toString().padStart(2, "0"),
      random: Math.random().toString(36).substring(2, 8),
      uuid: this.#generateUUID(),
    };
  }

  #generateUUID() {
    return "xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx".replace(/[xy]/g, (c) => {
      const r = (Math.random() * 16) | 0;
      const v = c === "x" ? r : (r & 0x3) | 0x8;
      return v.toString(16);
    });
  }

  set(name, value) {
    this.#variables.set(name, value);
  }

  get(name) {
    if (this.#variables.has(name)) {
      return this.#variables.get(name);
    }
    if (name in this.#builtins) {
      return this.#builtins[name];
    }
    return undefined;
  }

  has(name) {
    return this.#variables.has(name) || name in this.#builtins;
  }

  getAll() {
    const all = { ...this.#builtins };
    this.#variables.forEach((value, key) => {
      all[key] = value;
    });
    return all;
  }

  resolve(value) {
    if (typeof value !== "string") {
      return value;
    }

    return value.replace(/\$\{([^}]+)\}/g, (match, varName) => {
      const trimmed = varName.trim();

      if (this.#variables.has(trimmed)) {
        return this.#variables.get(trimmed);
      }

      if (trimmed in this.#builtins) {
        return this.#builtins[trimmed];
      }

      if (trimmed.startsWith("env.")) {
        const envVar = trimmed.substring(4);
        return process.env[envVar] || match;
      }

      return match;
    });
  }

  resolveObject(obj) {
    if (obj === null || obj === undefined) {
      return obj;
    }

    if (typeof obj === "string") {
      return this.resolve(obj);
    }

    if (Array.isArray(obj)) {
      return obj.map((item) => this.resolveObject(item));
    }

    if (typeof obj === "object") {
      const result = {};
      for (const [key, value] of Object.entries(obj)) {
        result[key] = this.resolveObject(value);
      }
      return result;
    }

    return obj;
  }

  evaluateCondition(condition) {
    const resolved = this.resolve(condition);

    if (resolved === "true" || resolved === true) return true;
    if (resolved === "false" || resolved === false) return false;

    if (typeof resolved === "string") {
      if (/^[\d.]+$/.test(resolved)) {
        return parseFloat(resolved) !== 0;
      }
      return resolved.length > 0;
    }

    return Boolean(resolved);
  }

  parseExpression(expr) {
    const resolved = this.resolve(expr);

    try {
      return JSON.parse(resolved);
    } catch {
      return resolved;
    }
  }
}

module.exports = { VariableResolver };
