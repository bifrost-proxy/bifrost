import type { RuleFile } from '../../../types';

export interface TreeNode {
  name: string;
  fullPath: string;
  isDirectory: boolean;
  children?: TreeNode[];
  rule?: RuleFile;
}

export function buildTree(rules: RuleFile[]): TreeNode {
  const root: TreeNode = {
    name: '',
    fullPath: '',
    isDirectory: true,
    children: [],
  };

  rules.forEach((rule) => {
    const parts = rule.name.split('/');
    let currentNode = root;
    let currentPath = '';

    parts.forEach((part: string, index: number) => {
      const isLastPart = index === parts.length - 1;
      currentPath = currentPath ? `${currentPath}/${part}` : part;

      let childNode = currentNode.children?.find((child) => child.name === part);

      if (!childNode) {
        childNode = {
          name: part,
          fullPath: currentPath,
          isDirectory: !isLastPart,
          children: isLastPart ? undefined : [],
          rule: isLastPart ? rule : undefined,
        };

        if (!currentNode.children) {
          currentNode.children = [];
        }
        currentNode.children.push(childNode);

        currentNode.children.sort((a, b) => {
          if (a.isDirectory !== b.isDirectory) {
            return a.isDirectory ? -1 : 1;
          }
          return a.name.localeCompare(b.name);
        });
      } else if (isLastPart) {
        childNode.rule = rule;
      }

      currentNode = childNode;
    });
  });

  return root;
}
