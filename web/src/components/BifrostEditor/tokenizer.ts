import { languages } from 'monaco-editor';

const config: languages.LanguageConfiguration = {
  comments: {
    lineComment: '#',
  },
  brackets: [
    ['{', '}'],
    ['[', ']'],
    ['(', ')'],
    ['${', '}'],
  ],
  autoClosingPairs: [
    { open: '"', close: '"' },
    { open: "'", close: "'" },
    { open: '`', close: '`' },
    { open: '${', close: '}' },
    { open: '{', close: '}' },
    { open: '[', close: ']' },
    { open: '(', close: ')' },
    { open: '```', close: '```', notIn: ['comment'] },
  ],
  folding: {
    offSide: true,
    markers: {
      start: /^\s*line`\s*$/,
      end: /^\s*`\s*$/,
    },
  },
};

/* eslint-disable no-useless-escape */
const language: languages.IMonarchLanguage = {
  defaultToken: 'invalid',
  tokenPostfix: '.bifrost',
  ignoreCase: false,
  unicode: true,

  comment: /#.*$/,
  regexpctl: /[(){}\[\]\$\^|\-*+?\.]/,
  regexpesc:
    /\\(?:[bBdDfnrstvwWn0\\\/]|@regexpctl|c[A-Z]|x[0-9a-fA-F]{2}|u[0-9a-fA-F]{4})/,

  scheme: /(http|ws)s?|tunnel/i,

  tokenizer: {
    root: [
      { include: '@whitespace' },
      { include: '@comment' },

      [
        /^(line)(`)$/,
        [
          { token: 'keyword' },
          { token: 'delimiter.bracket', bracket: '@open', next: '@lineblock' },
        ],
      ],

      [
        /\/(?=([^\\\/]|\\.)+\/([dgimsuy]*)(\s*))/,
        {
          token: 'regexp',
          bracket: '@open',
          next: '@regexp',
        },
      ],

      [
        /(\s*)(@[\w.\-]+)(\/[^#]*)(\s*|@comment)$/,
        ['', 'reference.user', 'reference.env', 'comment'],
      ],
      [
        /(^|\s)(@[^#\s]+)(\s*|@comment)$/,
        ['', 'reference.user', 'comment'],
      ],

      [
        /^(```)([\w\-]+)(\.)([\w/.]+)(.*)$/,
        [
          { token: 'delimiter.bracket', bracket: '@open' },
          { token: 'identifier.var' },
          { token: 'type.identifier' },
          {
            token: 'type.identifier',
            next: '@codeblock',
            nextEmbedded: '$4',
          },
          { token: 'string' },
        ],
      ],
      [
        /^(```)([\w\-]+)(.*)$/,
        [
          { token: 'delimiter.bracket', bracket: '@open' },
          {
            token: 'identifier.var',
            next: '@codeblock',
            nextEmbedded: 'yml',
          },
          { token: 'string' },
        ],
      ],

      [/([\w.\-]+:\/\/)([^#\s]*[?&=][^#\s]*)/, ['keyword', 'string']],
      [/[?&][^#\s]*/, 'string'],

      [
        /(\s*[^()#=\s]+\s*)(=)(\s*[^#]+)(\s*|@comment)$/,
        ['identifier.var', 'delimiter', 'attribute.value', 'comment'],
      ],
      [
        /(\$\{)([\w\-]+)(\})/,
        ['delimiter.bracket', 'identifier.var', 'delimiter.bracket'],
      ],

      [
        /([\w.\-]+:\/\/)([{\[(]|\$\{)([^#)\]\}]*)([)\]}])/,
        ['keyword', 'delimiter.bracket', 'identifier.var', 'delimiter.bracket'],
      ],
      [
        /([\w.\-]+:\/\/)(\{)([^#()[\]}]*)(\})/,
        ['keyword', 'delimiter.bracket', 'identifier.var', 'delimiter.bracket'],
      ],
      [
        /([\w.\-]+:\/\/)(`)(\$\{)([^#()[\]}`]*)(\})(`)/,
        [
          'keyword',
          'string',
          'delimiter.bracket',
          'identifier.var',
          'delimiter.bracket',
          'string',
        ],
      ],

      [/([\w.\-]+:\/\/)([^#\s]*)/, ['keyword', 'string']],

      [/(@scheme:\/\/)?[\w@:/?*.-]+/, 'scheme'],

      { include: '@bracket' },
    ],

    whitespace: [[/\s+/, '']],
    comment: [[/@comment/, 'comment']],
    bracket: [[/[{}[\]()]|\$\{/, 'delimiter.bracket']],

    regexp: [
      [
        /(\{)(\d+(?:,\d*)?)(\})/,
        [
          'regexp.escape.control',
          'regexp.escape.control',
          'regexp.escape.control',
        ],
      ],
      [
        /(\[)(\^?)(?=(?:[^\]\\\/]|\\.)+)/,
        [
          { token: 'regexp.escape.control' },
          { token: 'regexp.escape.control', next: '@regexrange' },
        ],
      ],
      [/(\()(\?:|\?=|\?!)/, ['regexp.escape.control', 'regexp.escape.control']],
      [/[()]/, 'regexp.escape.control'],
      [/@regexpctl/, 'regexp.escape.control'],
      [/[^\\\/]/, 'regexp'],
      [/@regexpesc/, 'regexp.escape'],
      [/\\\./, 'regexp.invalid'],
      [
        /(\/)([dgimsuy]*)/,
        [
          { token: 'regexp', bracket: '@close', next: '@pop' },
          { token: 'keyword.other' },
        ],
      ],
    ],
    regexrange: [
      [/-/, 'regexp.escape.control'],
      [/\^/, 'regexp.invalid'],
      [/@regexpesc/, 'regexp.escape'],
      [/[^\]]/, 'regexp'],
      [
        /\]/,
        {
          token: 'regexp.escape.control',
          next: '@pop',
          bracket: '@close',
        },
      ],
    ],

    codeblock: [
      [
        /```\s*$/,
        {
          token: 'delimiter.bracket',
          bracket: '@close',
          next: '@pop',
          nextEmbedded: '@pop',
        },
      ],
      [/[^`]+/, 'string'],
    ],

    lineblock: [
      [
        /^`$/,
        {
          token: 'delimiter.bracket',
          bracket: '@close',
          next: '@pop',
        },
      ],
      { include: '@comment' },
      [
        /(includeFilter:\/\/)(.*)/,
        ['keyword', 'string'],
      ],
      [
        /(excludeFilter:\/\/)(.*)/,
        ['keyword', 'string'],
      ],
      [
        /(lineProps:\/\/)(.*)/,
        ['keyword', 'string'],
      ],
      [
        /([\w.\-]+:\/\/)([^#\s]*[?&=][^#\s]*)/,
        ['keyword', 'string'],
      ],
      [/[?&][^#\s]*/, 'string'],
      [
        /([\w.\-]+:\/\/)([^#\s]*)/,
        ['keyword', 'string'],
      ],
      [/`[^`]+`/, 'scheme'],
      [/(@scheme:\/\/)?[\w@:/?*.\-]+/, 'scheme'],
      { include: '@whitespace' },
      [/./, ''],
    ],
  },
};
/* eslint-enable no-useless-escape */

export default {
  config,
  language,
};
