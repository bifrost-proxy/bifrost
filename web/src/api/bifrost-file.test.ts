import { describe, expect, it } from 'vitest';

import {
  formatBifrostFileError,
  formatImportSuccessMessage,
  getEmptyExportMessage,
  getExportItemCount,
  getImportedItemCount,
  type ImportResponse,
} from './bifrost-file';

describe('bifrost file import helpers', () => {
  it('reports zero network imports as an empty package', () => {
    const result: ImportResponse = {
      success: true,
      file_type: 'network',
      data: { record_count: 0 },
    };

    expect(getImportedItemCount(result)).toBe(0);
    expect(formatImportSuccessMessage(result, 'empty.bifrost')).toBe(
      'empty.bifrost contains no network items to import',
    );
  });

  it('includes the actual imported item count for non-empty packages', () => {
    const result: ImportResponse = {
      success: true,
      file_type: 'network',
      data: { record_count: 2 },
    };

    expect(getImportedItemCount(result)).toBe(2);
    expect(formatImportSuccessMessage(result, 'traffic.bifrost')).toBe(
      'Imported traffic.bifrost successfully (2 items)',
    );
  });

  it('extracts backend error messages from axios responses', () => {
    const error = {
      isAxiosError: true,
      message: 'Request failed with status code 400',
      response: {
        data: {
          error: 'Network file contains 0 records; nothing to import.',
        },
      },
    };

    expect(formatBifrostFileError(error)).toBe(
      'Network file contains 0 records; nothing to import.',
    );
  });
});

describe('bifrost file export helpers', () => {
  it('blocks empty network exports before they can create a package', () => {
    const request = { record_ids: [], include_body: true };

    expect(getExportItemCount('network', request)).toBe(0);
    expect(getEmptyExportMessage('network', request)).toBe(
      'Select at least one Network record before exporting a .bifrost file',
    );
  });

  it('allows network exports with at least one selected record', () => {
    const request = { record_ids: ['REQ-1'], include_body: true };

    expect(getExportItemCount('network', request)).toBe(1);
    expect(getEmptyExportMessage('network', request)).toBeUndefined();
  });
});
