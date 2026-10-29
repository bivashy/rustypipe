use std::fmt::Debug;

use crate::{
    client::{ClientType, MapJsonResponse, MapRespCtx, MapRespOptions, RustyPipeQuery},
    error::{Error, ExtractionError},
    json::{JsonDoc, yt_two_column_list_items},
    model::{
        paginator::{ContinuationEndpoint, Paginator},
        ChannelItem, HistoryItem, Playlist, PlaylistItem, VideoItem, YouTubeItem,
    },
    request_body::ytbody,
    serializer::MapResult,
};

use super::response::{
    tv::{TvFeedJson, TvPlaylistJson},
    YouTubeListItem, YouTubeListMapper,
};

use super::pagination::ContinuationJson;

#[derive(Debug)]
struct HistoryJson;

impl RustyPipeQuery {
    /// Returns `true` if only OAuth credentials (TV client) are available
    /// and no authentication cookie is set.
    fn use_tv_auth(&self) -> bool {
        !self.auth_enabled(ClientType::Desktop) && self.auth_enabled(ClientType::Tv)
    }

    /// Get a list of videos from YouTube which the current user recently played
    ///
    /// Requires authentication cookies or OAuth login. If both are set, the cookies are used.
    ///
    /// When fetched with OAuth (TV client), the history items do not have a playback date
    /// (`playback_date` and `playback_date_txt` are `None`) and channel ids are not
    /// available for all videos (`channel.id` may be empty).
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn history(&self) -> Result<Paginator<HistoryItem<VideoItem>>, Error> {
        if self.use_tv_auth() {
            let request_body = ytbody!({
                "browseId": "FEhistory",
            });
            return self
                .clone()
                .authenticated()
                .execute_request::<TvFeedJson, _, _>(
                    ClientType::Tv,
                    "history",
                    "",
                    "browse",
                    &request_body,
                )
                .await;
        }

        let request_body = ytbody!({
            "browseId": "FEhistory",
        });

        self.clone()
            .authenticated()
            .execute_request::<HistoryJson, _, _>(
                ClientType::Desktop,
                "history",
                "",
                "browse",
                &request_body,
            )
            .await
    }

    /// Get more YouTube history items from the given continuation token (TV client)
    pub(crate) async fn history_continuation_tv(
        &self,
        ctoken: &str,
        visitor_data: Option<&str>,
    ) -> Result<Paginator<HistoryItem<VideoItem>>, Error> {
        let request_body = ytbody!({
            "continuation": ctoken,
        });

        self.clone()
            .authenticated()
            .execute_request_ctx::<TvFeedJson, _, _>(
                ClientType::Tv,
                "history_continuation",
                ctoken,
                "browse",
                &request_body,
                MapRespOptions {
                    visitor_data,
                    ..Default::default()
                },
            )
            .await
    }

    /// Get more YouTube history items from the given continuation token
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn history_continuation<S: AsRef<str> + Debug>(
        &self,
        ctoken: S,
        visitor_data: Option<&str>,
    ) -> Result<Paginator<HistoryItem<VideoItem>>, Error> {
        let ctoken = ctoken.as_ref();
        let request_body = ytbody!({
            "continuation": ctoken,
        });

        self.clone()
            .authenticated()
            .execute_request_ctx::<ContinuationJson, _, _>(
                ClientType::Desktop,
                "history_continuation",
                ctoken,
                "browse",
                &request_body,
                MapRespOptions {
                    visitor_data,
                    ..Default::default()
                },
            )
            .await
    }

    /// Search the YouTube playback history of the current user
    ///
    /// Requires authentication cookies (OAuth login is not supported for this function).
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn history_search<S: AsRef<str> + Debug>(
        &self,
        query: S,
    ) -> Result<Paginator<HistoryItem<VideoItem>>, Error> {
        let query = query.as_ref();
        let request_body = ytbody!({
            "browseId": "FEhistory",
            "query": query,
        });

        self.clone()
            .authenticated()
            .execute_request::<HistoryJson, _, _>(
                ClientType::Desktop,
                "history_search",
                query,
                "browse",
                &request_body,
            )
            .await
    }

    /// Get a list of channels the current user subscribed to from YouTube
    ///
    /// Requires authentication cookies or OAuth login. If both are set, the cookies are used.
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn subscriptions(&self) -> Result<Paginator<ChannelItem>, Error> {
        if self.use_tv_auth() {
            let request_body = ytbody!({
                "browseId": "FEchannels",
            });
            return self
                .clone()
                .authenticated()
                .execute_request::<TvFeedJson, _, _>(
                    ClientType::Tv,
                    "subscriptions",
                    "",
                    "browse",
                    &request_body,
                )
                .await;
        }

        self.clone()
            .authenticated()
            .continuation(
                "4qmFsgIqEgpGRWNoYW5uZWxzGgRrQUlDmgIVYnJvd3NlLWZlZWRGRWNoYW5uZWxz",
                ContinuationEndpoint::Browse,
                None,
            )
            .await
    }

    /// Get the YouTube subscription feed of the current user
    ///
    /// Requires authentication cookies or OAuth login. If both are set, the cookies are used.
    ///
    /// When fetched with OAuth (TV client), channel ids are not available for all videos
    /// (`channel.id` may be empty).
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn subscription_feed(&self) -> Result<Paginator<VideoItem>, Error> {
        if self.use_tv_auth() {
            let request_body = ytbody!({
                "browseId": "FEsubscriptions",
            });
            return self
                .clone()
                .authenticated()
                .execute_request::<TvFeedJson, _, _>(
                    ClientType::Tv,
                    "subscription_feed",
                    "",
                    "browse",
                    &request_body,
                )
                .await;
        }

        let request_body = ytbody!({
            "browseId": "FEsubscriptions",
        });

        self.clone()
            .authenticated()
            .execute_request::<HistoryJson, _, _>(
                ClientType::Desktop,
                "subscription_feed",
                "",
                "browse",
                &request_body,
            )
            .await
    }

    /// Get the YouTube home feed (recommendations) of the current user
    ///
    /// The items of all shelves (videos, shorts, playlists/mixes) are returned
    /// as a flat list, in the order they appear on the page.
    ///
    /// Requires OAuth login (the feed is fetched with the TV client).
    ///
    /// Note that channel ids are not available from the TV client
    /// (`channel.id` of the videos is empty).
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn home_feed(&self) -> Result<Paginator<YouTubeItem>, Error> {
        let request_body = ytbody!({
            "browseId": "default",
        });

        self.clone()
            .authenticated()
            .execute_request::<TvFeedJson, _, _>(
                ClientType::Tv,
                "home_feed",
                "",
                "browse",
                &request_body,
            )
            .await
    }

    /// Get a list of YouTube playlists the current user added to their library
    ///
    /// Requires authentication cookies or OAuth login. If both are set, the cookies are used.
    pub async fn saved_playlists(&self) -> Result<Paginator<PlaylistItem>, Error> {
        if self.use_tv_auth() {
            let request_body = ytbody!({
                "browseId": "FEplaylist_aggregation",
            });
            return self
                .clone()
                .authenticated()
                .execute_request::<TvFeedJson, _, _>(
                    ClientType::Tv,
                    "saved_playlists",
                    "",
                    "browse",
                    &request_body,
                )
                .await;
        }

        self.clone()
            .authenticated()
            .continuation(
                "4qmFsgJFEhZGRXBsYXlsaXN0X2FnZ3JlZ2F0aW9uGgRxQUlDmgIkNjc5MjVhZTYtMDAwMC0yYzQyLWFjMjItM2MyODZkNDI1MTQy",
                ContinuationEndpoint::Browse,
                None,
            )
            .await
    }

    /// Fetch a private playlist of the logged-in user (`LL`, `WL`) using the TV client
    async fn playlist_tv(&self, playlist_id: &str) -> Result<Playlist, Error> {
        let request_body = ytbody!({
            "browseId": format!("VL{playlist_id}"),
        });

        self.clone()
            .authenticated()
            .execute_request::<TvPlaylistJson, _, _>(
                ClientType::Tv,
                "playlist",
                playlist_id,
                "browse",
                &request_body,
            )
            .await
            .map_err(crate::util::map_internal_playlist_err)
    }

    /// Get all liked videos of the logged-in user
    ///
    /// Requires authentication cookies or OAuth login. If both are set, the cookies are used.
    ///
    /// When fetched with OAuth (TV client), the playlist has no thumbnail, description or
    /// update date. Channel ids are not available (`channel.id` of the playlist and
    /// its videos is empty).
    pub async fn liked_videos(&self) -> Result<Playlist, Error> {
        if self.use_tv_auth() {
            return self.playlist_tv("LL").await;
        }

        self.clone()
            .authenticated()
            .playlist("LL")
            .await
            .map_err(crate::util::map_internal_playlist_err)
    }

    /// Get the "Watch later" playlist of the logged-in user
    ///
    /// Requires authentication cookies or OAuth login. If both are set, the cookies are used.
    ///
    /// When fetched with OAuth (TV client), the playlist has no thumbnail, description or
    /// update date. Channel ids are not available (`channel.id` of the playlist and
    /// its videos is empty).
    pub async fn watch_later(&self) -> Result<Playlist, Error> {
        if self.use_tv_auth() {
            return self.playlist_tv("WL").await;
        }

        self.clone()
            .authenticated()
            .playlist("WL")
            .await
            .map_err(crate::util::map_internal_playlist_err)
    }
}

impl MapJsonResponse<Paginator<HistoryItem<VideoItem>>> for HistoryJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Paginator<HistoryItem<VideoItem>>>, ExtractionError> {
        json.with_root(|root| {
            let items = yt_two_column_list_items(&root)?;
            let (sections, warnings) = items.deserialize_items_lossy::<YouTubeListItem>();

            let mut map_res = MapResult {
                warnings,
                ..Default::default()
            };
            let mut ctoken = None;

            for item in sections {
                match item {
                    YouTubeListItem::ItemSectionRenderer { header, contents } => {
                        let mut mapper = YouTubeListMapper::<VideoItem>::new(ctx.lang);
                        mapper.map_response(contents);
                        mapper.conv_history_items(
                            header.map(|h| h.item_section_header_renderer.title),
                            ctx.utc_offset,
                            &mut map_res,
                        );
                    }
                    YouTubeListItem::ContinuationItemRenderer(ep) if ctoken.is_none() => {
                        ctoken = ep.continuation_endpoint.into_token();
                    }
                    YouTubeListItem::ContinuationItemRenderer(_) => {}
                    _ => {}
                }
            }

            Ok(MapResult {
                c: Paginator::new_ext(
                    None,
                    map_res.c,
                    ctoken,
                    ctx.visitor_data.map(str::to_owned),
                    ContinuationEndpoint::Browse,
                    true,
                ),
                warnings: map_res.warnings,
            })
        })
    }
}

impl MapJsonResponse<Paginator<VideoItem>> for HistoryJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Paginator<VideoItem>>, ExtractionError> {
        json.with_root(|root| {
            let items = yt_two_column_list_items(&root)?;
            let mut mapper = YouTubeListMapper::<VideoItem>::new(ctx.lang);
            mapper.map_response_node(&items);

            Ok(MapResult {
                c: Paginator::new_ext(
                    None,
                    mapper.items,
                    mapper.ctoken,
                    ctx.visitor_data.map(str::to_owned),
                    ContinuationEndpoint::Browse,
                    true,
                ),
                warnings: mapper.warnings,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use path_macro::path;

    use crate::util::tests::TESTFILES;

    use super::*;

    #[test]
    fn map_history() {
        let json_path = path!(*TESTFILES / "userdata" / "history.json");
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Paginator<HistoryItem<VideoItem>>> =
            HistoryJson::map_json_response(&json, &MapRespCtx::test("")).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        insta::assert_ron_snapshot!(map_res.c, {
            ".items[].playback_date" => "[date]",
        });
    }

    #[rstest::rstest]
    #[case::initial("home_feed", true)]
    #[case::continuation("home_feed_continuation", true)]
    #[case::subscription_feed("tv_subscription_feed", true)]
    #[case::subscription_feed_continuation("tv_subscription_feed_continuation", true)]
    #[case::subscriptions("tv_subscriptions", false)]
    #[case::saved_playlists("tv_saved_playlists", true)]
    #[case::saved_playlists_continuation("tv_saved_playlists_continuation", true)]
    fn map_tv_feed(#[case] name: &str, #[case] has_ctoken: bool) {
        let json_path = path!(*TESTFILES / "userdata" / format!("{name}.json"));
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Paginator<YouTubeItem>> =
            TvFeedJson::map_json_response(&json, &MapRespCtx::test("")).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        assert_eq!(map_res.c.ctoken.is_some(), has_ctoken);
        assert!(!map_res.c.items.is_empty());
        insta::assert_ron_snapshot!(format!("map_{name}"), map_res.c, {
            ".items[].publish_date" => "[date]",
        });
    }

    #[rstest::rstest]
    #[case::initial("tv_history")]
    #[case::continuation("tv_history_continuation")]
    fn map_tv_history(#[case] name: &str) {
        let json_path = path!(*TESTFILES / "userdata" / format!("{name}.json"));
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Paginator<HistoryItem<VideoItem>>> =
            TvFeedJson::map_json_response(&json, &MapRespCtx::test("")).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        assert!(map_res.c.ctoken.is_some());
        assert!(!map_res.c.items.is_empty());
        assert!(map_res.c.items.iter().all(|i| i.playback_date.is_none()));
        insta::assert_ron_snapshot!(format!("map_{name}"), map_res.c, {
            ".items[].item.publish_date" => "[date]",
        });
    }

    #[rstest::rstest]
    #[case::liked_videos("LL", "tv_liked_videos")]
    #[case::watch_later("WL", "tv_watch_later")]
    fn map_tv_playlist(#[case] id: &str, #[case] name: &str) {
        let json_path = path!(*TESTFILES / "userdata" / format!("{name}.json"));
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Playlist> =
            TvPlaylistJson::map_json_response(&json, &MapRespCtx::test(id)).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        assert_eq!(map_res.c.id, id);
        assert!(map_res.c.videos.ctoken.is_some());
        assert!(map_res.c.video_count > 0);
        insta::assert_ron_snapshot!(format!("map_{name}"), map_res.c, {
            ".videos.items[].publish_date" => "[date]",
        });
    }

    #[rstest::rstest]
    #[case::liked_videos("tv_liked_videos_continuation")]
    #[case::watch_later("tv_watch_later_continuation")]
    fn map_tv_playlist_continuation(#[case] name: &str) {
        let json_path = path!(*TESTFILES / "userdata" / format!("{name}.json"));
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Paginator<VideoItem>> =
            TvFeedJson::map_json_response(&json, &MapRespCtx::test("")).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        assert!(map_res.c.ctoken.is_some());
        assert!(!map_res.c.items.is_empty());
        insta::assert_ron_snapshot!(format!("map_{name}"), map_res.c, {
            ".items[].publish_date" => "[date]",
        });
    }

    #[test]
    fn map_subscription_feed() {
        let json_path = path!(*TESTFILES / "userdata" / "subscription_feed.json");
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Paginator<VideoItem>> =
            HistoryJson::map_json_response(&json, &MapRespCtx::test("")).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        insta::assert_ron_snapshot!(map_res.c, {
            ".items[].publish_date" => "[date]",
        });
    }
}
