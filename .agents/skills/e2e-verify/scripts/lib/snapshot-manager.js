class SnapshotNode {
  constructor(node, id) {
    this.id = id;
    this.role = node.role || "";
    this.name = node.name || "";
    this.value = node.value;
    this.description = node.description;
    this.disabled = node.disabled;
    this.focused = node.focused;
    this.checked = node.checked;
    this.selected = node.selected;
    this.expanded = node.expanded;
    this.level = node.level;
    this.valuemin = node.valuemin;
    this.valuemax = node.valuemax;
    this.valuetext = node.valuetext;
    this.autocomplete = node.autocomplete;
    this.haspopup = node.haspopup;
    this.invalid = node.invalid;
    this.orientation = node.orientation;
    this.multiselectable = node.multiselectable;
    this.keyshortcuts = node.keyshortcuts;
    this.roledescription = node.roledescription;
    this.pressed = node.pressed;
    this.required = node.required;
    this.readonly = node.readonly;
    this.elementHandle = node.elementHandle;
    this.children = [];
    this.backendNodeId = node.backendNodeId;
    this.componentName = node.componentName || null;
  }
}

class SnapshotManager {
  #page = null;
  #snapshot = null;
  #nextSnapshotId = 1;
  #idCounter = 0;

  setPage(page) {
    this.#page = page;
  }

  getPage() {
    if (!this.#page) {
      throw new Error("No page set. Call setPage() first.");
    }
    return this.#page;
  }

  get nodeCount() {
    return this.#snapshot ? this.#snapshot.idToNode.size : 0;
  }

  #getReactComponentScript = `function() {
    const FunctionComponent = 0;
    const ClassComponent = 1;
    const HostComponent = 5;
    
    const fiberKey = Object.keys(this).find(k => 
      k.startsWith('__reactFiber$') || 
      k.startsWith('__reactInternalInstance$')
    );
    if (!fiberKey) return null;
    
    const fiber = this[fiberKey];
    if (!fiber) return null;
    
    // 排除 UI 框架容器组件和 React 内部组件
    const skipComponents = new Set([
      // Layout 容器组件
      'Layout', 'Header', 'Content', 'Footer', 'Sider',
      'Row', 'Col', 'Space', 'Grid', 'Flex',
      // React 内部组件
      'Fragment', 'Suspense', 'Provider', 'Consumer', 'Context',
      'LocaleConsumer', 'LocaleProvider', 'ConfigProvider', 'ThemeProvider',
      // 通用容器
      'Wrapper', 'Container', 'Box', 'Div', 'Span',
      'Basic', 'Adapter', 'Portal',
      // 路由组件
      'Routes', 'Router', 'BrowserRoutes', 'Switch', 'Route',
      'Outlet', 'RemoteComponent',
      // Semi UI 内部组件
      'RadioInner', 'CheckboxInner', 'InputInner',
      'PopoverInner', 'TooltipInner', 'DropdownInner'
    ]);
    
    const getComponentName = (f) => {
      if (!f || !f.type || typeof f.type === 'string') return null;
      const name = f.type.displayName || f.type.name;
      if (name && !name.startsWith('_') && name.length > 1) return name;
      return null;
    };
    
    const isBusinessComponent = (name) => {
      if (!name) return false;
      if (skipComponents.has(name)) return false;
      // 匿名组件或单字母组件跳过
      if (name === 'Anonymous' || name.length <= 1) return false;
      // 以小写字母开头的跳过（通常是内部组件）
      if (name[0] === name[0].toLowerCase()) return false;
      return true;
    };
    
    const getFirstDOMChild = (f) => {
      if (!f) return null;
      if (f.stateNode instanceof Element) return f.stateNode;
      let child = f.child;
      while (child) {
        if (child.stateNode instanceof Element) return child.stateNode;
        const result = getFirstDOMChild(child);
        if (result) return result;
        child = child.sibling;
      }
      return null;
    };
    
    // 策略 1: 当前 fiber 是函数/类组件
    if (fiber.tag === FunctionComponent || fiber.tag === ClassComponent) {
      const name = getComponentName(fiber);
      if (name && isBusinessComponent(name)) return { name, isRoot: true };
    }
    
    // 策略 2: 使用 _debugOwner 检查是否是组件的根 DOM（精确匹配）
    if (fiber._debugOwner) {
      const owner = fiber._debugOwner;
      const name = getComponentName(owner);
      if (name && isBusinessComponent(name)) {
        const rootDOM = getFirstDOMChild(owner);
        if (rootDOM === this) {
          return { name, isRoot: true };
        }
      }
    }
    
    // 策略 3: 向上遍历找最近的业务组件（不要求是 root）
    let nearestComponent = null;
    let current = fiber.return;
    while (current) {
      if (current.tag === FunctionComponent || current.tag === ClassComponent) {
        const name = getComponentName(current);
        if (name && isBusinessComponent(name)) {
          // 检查是否是精确的 root
          const rootDOM = getFirstDOMChild(current);
          if (rootDOM === this) {
            return { name, isRoot: true };
          }
          // 记录最近的业务组件
          if (!nearestComponent) {
            nearestComponent = name;
          }
        }
      }
      current = current.return;
    }
    
    // 返回最近的业务组件（非 root）
    if (nearestComponent) {
      return { name: nearestComponent, isRoot: false };
    }
    
    return null;
  }`;

  async #collectReactComponentsByCDP(page) {
    const client = await page.target().createCDPSession();

    try {
      await client.send("DOM.enable");
      await client.send("Accessibility.enable");
      const { nodes } = await client.send("Accessibility.getFullAXTree");

      const backendIdToComponent = new Map();
      const axNodeIdToBackendId = new Map();

      for (const node of nodes) {
        if (node.backendDOMNodeId) {
          axNodeIdToBackendId.set(node.nodeId, node.backendDOMNodeId);
        }
      }

      const processedBackendIds = new Set();

      for (const node of nodes) {
        if (!node.backendDOMNodeId) continue;
        if (processedBackendIds.has(node.backendDOMNodeId)) continue;
        processedBackendIds.add(node.backendDOMNodeId);

        try {
          const { object } = await client.send("DOM.resolveNode", {
            backendNodeId: node.backendDOMNodeId,
          });

          if (object && object.objectId) {
            const result = await client.send("Runtime.callFunctionOn", {
              objectId: object.objectId,
              functionDeclaration: this.#getReactComponentScript,
              returnByValue: true,
            });

            if (result.result && result.result.value) {
              const { name } = result.result.value;
              if (name) {
                backendIdToComponent.set(node.backendDOMNodeId, name);
              }
            }

            await client.send("Runtime.releaseObject", {
              objectId: object.objectId,
            });
          }
        } catch {}
      }

      return { backendIdToComponent, axNodes: nodes, axNodeIdToBackendId };
    } finally {
      await client.send("Accessibility.disable");
      await client.send("DOM.disable");
      await client.detach();
    }
  }

  async createSnapshot(options = {}) {
    const page = this.getPage();
    const { verbose = false, useCDP = true } = options;

    let backendIdToComponent = new Map();
    let cdpAxNodes = null;

    if (useCDP) {
      try {
        const cdpResult = await this.#collectReactComponentsByCDP(page);
        backendIdToComponent = cdpResult.backendIdToComponent;
        cdpAxNodes = cdpResult.axNodes;
      } catch (err) {
        console.warn(
          "CDP component collection failed, falling back:",
          err.message,
        );
      }
    }

    const rootNode = await page.accessibility.snapshot({
      interestingOnly: !verbose,
    });

    if (!rootNode) {
      throw new Error(
        "Failed to create accessibility snapshot - page may be empty or not loaded.",
      );
    }

    const snapshotId = this.#nextSnapshotId++;
    this.#idCounter = 0;

    const idToNode = new Map();
    const idToHandle = new Map();

    const cdpNodeMap = new Map();
    if (cdpAxNodes) {
      for (const cdpNode of cdpAxNodes) {
        const key = this.#makeAxNodeKey(cdpNode);
        if (key) {
          cdpNodeMap.set(key, cdpNode);
        }
      }
    }

    const processNode = async (axNode, parent = null) => {
      const uid = `e${snapshotId}_${this.#idCounter++}`;

      let componentName = null;

      const nodeKey = this.#makeAxNodeKey(axNode);
      if (nodeKey && cdpNodeMap.has(nodeKey)) {
        const cdpNode = cdpNodeMap.get(nodeKey);
        if (cdpNode.backendDOMNodeId) {
          componentName =
            backendIdToComponent.get(cdpNode.backendDOMNodeId) || null;
        }
      }

      const nodeData = { ...axNode, componentName };
      const node = new SnapshotNode(nodeData, uid);

      idToNode.set(uid, node);

      if (axNode.elementHandle) {
        idToHandle.set(uid, axNode.elementHandle);
      }

      if (axNode.children) {
        for (const child of axNode.children) {
          const childNode = await processNode(child, node);
          node.children.push(childNode);
        }
      }

      return node;
    };

    const rootWithIds = await processNode(rootNode);

    this.#snapshot = {
      root: rootWithIds,
      idToNode,
      idToHandle,
      snapshotId: String(snapshotId),
      verbose,
      timestamp: Date.now(),
    };

    return this;
  }

  #makeAxNodeKey(node) {
    const role = node.role?.value || node.role || "";
    const name = node.name?.value || node.name || "";
    if (!role) return null;
    return `${role}|${name}`;
  }

  getSnapshot() {
    return this.#snapshot;
  }

  hasSnapshot() {
    return this.#snapshot !== null;
  }

  getElementByUid(uid) {
    if (!this.#snapshot) {
      throw new Error("No snapshot available. Call createSnapshot() first.");
    }
    return this.#snapshot.idToNode.get(uid);
  }

  async getHandleByUid(uid) {
    if (!this.#snapshot) {
      throw new Error("No snapshot available. Call createSnapshot() first.");
    }

    const handle = this.#snapshot.idToHandle.get(uid);
    if (handle) {
      const isValid = typeof handle.click === "function";
      if (isValid) return handle;
    }

    const node = this.#snapshot.idToNode.get(uid);
    if (!node) {
      throw new Error(`Element with uid "${uid}" not found in snapshot.`);
    }

    const page = this.getPage();
    const element = await this.#findElementByNode(page, node);

    if (!element) {
      throw new Error(
        `Element with uid "${uid}" (role="${node.role}", name="${node.name}") no longer exists on page. ` +
          `Suggestion: Take a new snapshot with createSnapshot() and use the updated uid.`,
      );
    }

    if (typeof element.click !== "function") {
      throw new Error(
        `Element found for uid "${uid}" but it's not a valid ElementHandle. ` +
          `Got type: ${typeof element}, constructor: ${element?.constructor?.name}`,
      );
    }

    this.#snapshot.idToHandle.set(uid, element);
    return element;
  }

  async #findElementByNode(page, node) {
    if (!node.role && !node.name) {
      return null;
    }

    const ariaSelector = this.#buildAriaSelector(node);
    if (ariaSelector) {
      try {
        const element = await page.$(ariaSelector);
        if (element) return element;
      } catch {}
    }

    if (node.name) {
      const textSelector = `::-p-text(${node.name})`;
      try {
        const element = await page.$(textSelector);
        if (element) return element;
      } catch {}
    }

    return null;
  }

  #buildAriaSelector(node) {
    if (node.role && node.name) {
      const escapedName = node.name.replace(/"/g, '\\"');
      return `::-p-aria(${node.role}[name="${escapedName}"])`;
    }
    if (node.name) {
      const escapedName = node.name.replace(/"/g, '\\"');
      return `::-p-aria([name="${escapedName}"])`;
    }
    if (node.role) {
      return `::-p-aria(${node.role})`;
    }
    return null;
  }

  findNodes(predicate) {
    if (!this.#snapshot) {
      return [];
    }

    const results = [];
    const queue = [this.#snapshot.root];

    while (queue.length > 0) {
      const node = queue.shift();
      if (predicate(node)) {
        results.push(node);
      }
      queue.push(...node.children);
    }

    return results;
  }

  findNodesByRole(role) {
    return this.findNodes((node) => node.role === role);
  }

  findNodesByName(name, exact = false) {
    return this.findNodes((node) => {
      if (!node.name) return false;
      if (exact) return node.name === name;
      return node.name.toLowerCase().includes(name.toLowerCase());
    });
  }

  findNodesByText(text) {
    const lowerText = text.toLowerCase();
    return this.findNodes((node) => {
      const nodeName = (node.name || "").toLowerCase();
      const nodeValue = (node.value || "").toLowerCase();
      const nodeDesc = (node.description || "").toLowerCase();
      return (
        nodeName.includes(lowerText) ||
        nodeValue.includes(lowerText) ||
        nodeDesc.includes(lowerText)
      );
    });
  }

  findNodesByComponent(componentName, exact = false) {
    return this.findNodes((node) => {
      if (!node.componentName) return false;
      if (exact) return node.componentName === componentName;
      return node.componentName
        .toLowerCase()
        .includes(componentName.toLowerCase());
    });
  }

  findInteractiveElements() {
    const interactiveRoles = new Set([
      "button",
      "link",
      "textbox",
      "combobox",
      "listbox",
      "option",
      "checkbox",
      "radio",
      "slider",
      "spinbutton",
      "switch",
      "tab",
      "menuitem",
      "menuitemcheckbox",
      "menuitemradio",
      "searchbox",
    ]);

    return this.findNodes((node) => interactiveRoles.has(node.role));
  }

  formatSnapshot(options = {}) {
    if (!this.#snapshot) {
      return "No snapshot available.";
    }

    const { verbose = false, indent = 2 } = options;
    const lines = [];

    const formatNode = (node, depth = 0) => {
      const padding = " ".repeat(depth * indent);
      const attrs = [];

      if (node.role && node.role !== "none") {
        attrs.push(node.role);
      }

      if (node.name) {
        attrs.push(`"${node.name}"`);
      }

      attrs.push(`[${node.id}]`);

      if (node.componentName) {
        attrs.push(`<${node.componentName}>`);
      }

      if (verbose) {
        if (node.value !== undefined) attrs.push(`value="${node.value}"`);
        if (node.description) attrs.push(`desc="${node.description}"`);
        if (node.disabled) attrs.push("disabled");
        if (node.focused) attrs.push("focused");
        if (node.checked !== undefined) attrs.push(`checked=${node.checked}`);
        if (node.selected) attrs.push("selected");
        if (node.expanded !== undefined)
          attrs.push(`expanded=${node.expanded}`);
        if (node.required) attrs.push("required");
        if (node.readonly) attrs.push("readonly");
      }

      lines.push(padding + "- " + attrs.join(" "));

      for (const child of node.children) {
        formatNode(child, depth + 1);
      }
    };

    formatNode(this.#snapshot.root);
    return lines.join("\n");
  }

  toJSON() {
    if (!this.#snapshot) {
      return null;
    }

    const nodeToJSON = (node) => {
      const obj = {
        uid: node.id,
        role: node.role,
        name: node.name,
      };

      if (node.componentName) obj.componentName = node.componentName;
      if (node.value !== undefined) obj.value = node.value;
      if (node.description) obj.description = node.description;
      if (node.disabled) obj.disabled = node.disabled;
      if (node.focused) obj.focused = node.focused;
      if (node.checked !== undefined) obj.checked = node.checked;
      if (node.selected) obj.selected = node.selected;
      if (node.expanded !== undefined) obj.expanded = node.expanded;
      if (node.required) obj.required = node.required;
      if (node.readonly) obj.readonly = node.readonly;

      if (node.children.length > 0) {
        obj.children = node.children.map(nodeToJSON);
      }

      return obj;
    };

    return {
      snapshotId: this.#snapshot.snapshotId,
      timestamp: this.#snapshot.timestamp,
      nodeCount: this.nodeCount,
      root: nodeToJSON(this.#snapshot.root),
    };
  }

  getSuggestions(uid) {
    const node = this.getElementByUid(uid);
    if (node) {
      return [];
    }

    const suggestions = [];

    const uidMatch = uid.match(/^e(\d+)_(\d+)$/);
    if (uidMatch && this.#snapshot) {
      const [, snapshotId] = uidMatch;
      if (snapshotId !== this.#snapshot.snapshotId) {
        suggestions.push({
          type: "stale_snapshot",
          message: `The uid "${uid}" is from snapshot #${snapshotId}, but current snapshot is #${this.#snapshot.snapshotId}. Take a new snapshot.`,
        });
      }
    }

    const interactiveElements = this.findInteractiveElements();
    if (interactiveElements.length > 0) {
      suggestions.push(
        ...interactiveElements.slice(0, 5).map((n) => ({
          uid: n.id,
          role: n.role,
          name: n.name,
        })),
      );
    }

    return suggestions;
  }
}

module.exports = { SnapshotManager, SnapshotNode };
