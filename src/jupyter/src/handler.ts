import { URLExt } from '@jupyterlab/coreutils';

import { ServerConnection } from '@jupyterlab/services';

/**
 * Call the API extension
 *
 * @param endPoint API REST end point for the extension
 * @param body the body (if this is a POST)
 * @returns The response body interpreted as JSON
 */
export async function requestAPI<T>(endPoint = '', body: any = undefined): Promise<T> {
  // Make request to Jupyter API
  const settings = ServerConnection.makeSettings();
  const requestUrl = URLExt.join(
    settings.baseUrl,
    'jupyter-simlin', // API Namespace
    endPoint,
  );

  const init: RequestInit = {};
  if (body !== undefined) {
    // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
    init.body = typeof body === 'string' ? body : JSON.stringify(body);
    init.method = 'POST';

    const requestHeaders: HeadersInit = new Headers();
    requestHeaders.set('Content-Type', 'application/json');
    init.headers = requestHeaders;
  }

  let response: any;
  try {
    response = await ServerConnection.makeRequest(requestUrl, init, settings);
  } catch (error) {
    throw new ServerConnection.NetworkError(error);
  }

  // eslint-disable-next-line @typescript-eslint/no-unsafe-call,@typescript-eslint/no-unsafe-assignment
  let data: any = await response.text();

  if (data.length > 0) {
    try {
      // eslint-disable-next-line @typescript-eslint/no-unsafe-assignment
      data = JSON.parse(data);
    } catch (error) {
      console.log('Not a JSON response body.', response);
    }
  }

  if (!response.ok) {
    throw new ServerConnection.ResponseError(response, data.message || data);
  }

  // eslint-disable-next-line @typescript-eslint/no-unsafe-return
  return data;
}
