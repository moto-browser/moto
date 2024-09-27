/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at https://mozilla.org/MPL/2.0/. */

use std::future::Future;
use std::pin::Pin;

use headers::{ContentType, HeaderMapExt};
use http::StatusCode;
use servo::net::fetch::methods::{DoneChannel, FetchContext};
use servo::net::protocols::ProtocolHandler;
use servo::net_traits::request::Request;
use servo::net_traits::response::{Response, ResponseBody};
use servo::net_traits::ResourceFetchTiming;

#[derive(Default)]
pub struct UrlInfoProtocolHandler {}

// A simple protocol handler that displays information about the url itself.
impl ProtocolHandler for UrlInfoProtocolHandler {
    fn load(
        &self,
        request: &mut Request,
        _done_chan: &mut DoneChannel,
        _context: &FetchContext,
    ) -> Pin<Box<dyn Future<Output = Response> + Send>> {
        let url = request.current_url();

        let content = format!(
            r#"Full url: {url}
  scheme: {}
    path: {}
   query: {:?}"#,
            url.scheme(),
            url.path(),
            url.query()
        );
        let mut response = Response::new(url, ResourceFetchTiming::new(request.timing_type()));
        *response.body.lock().unwrap() = ResponseBody::Done(content.as_bytes().to_vec());
        response.headers.typed_insert(ContentType::text());
        response.status = Some((StatusCode::OK, "OK".to_string()));
        response.raw_status = Some((StatusCode::OK.as_u16(), b"OK".to_vec()));

        Box::pin(std::future::ready(response))
    }
}
