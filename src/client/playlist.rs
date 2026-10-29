use std::{borrow::Cow, convert::TryFrom, fmt::Debug};

use serde::Deserialize;
use serde_with::serde_as;
use time::OffsetDateTime;

use crate::{
    error::{Error, ExtractionError},
    json::{JsonDoc, JsonNode, yt_first_tab, yt_response_visitor_data, yt_thumbnails, ytq},
    model::{
        paginator::{ContinuationEndpoint, Paginator},
        richtext::RichText,
        ChannelId, ChannelTag, Playlist, VideoItem,
    },
    request_body::ytbody,
    serializer::text::{Text, TextComponent, TextComponents},
    util::{self, dictionary, timeago, TryRemove},
};

use super::{
    response,
    ClientType, MapJsonResponse, MapRespCtx, MapResult, RustyPipeQuery,
};

#[derive(Debug)]
struct PlaylistJson;

#[derive(Debug)]
struct MixJson;

/// Returns true if the ID belongs to an auto-generated YouTube Mix (`RD…`) which
/// cannot be fetched from the browse endpoint.
///
/// `RDCLAK…` IDs are regular (album) playlists.
fn is_mix_id(playlist_id: &str) -> bool {
    playlist_id.starts_with("RD") && !playlist_id.starts_with("RDCLAK")
}

impl RustyPipeQuery {
    /// Get a YouTube playlist
    ///
    /// Auto-generated YouTube Mixes (IDs starting with `RD`, e.g. from the home feed)
    /// are supported, too. These are infinite playlists, so only the first batch of
    /// videos is returned (the paginator is exhausted). Mixes have no channel,
    /// description, thumbnail or update date.
    #[tracing::instrument(skip(self), level = "error")]
    pub async fn playlist<S: AsRef<str> + Debug>(&self, playlist_id: S) -> Result<Playlist, Error> {
        let playlist_id = playlist_id.as_ref();
        if is_mix_id(playlist_id) {
            return self.playlist_mix(playlist_id).await;
        }

        let res = self.playlist_browse(playlist_id).await;
        match res {
            // Safety net for mix-like IDs which are not recognized by `is_mix_id`
            Err(Error::Extraction(ExtractionError::NotFound { ref msg, .. }))
                if msg.contains("unviewable") =>
            {
                self.playlist_mix(playlist_id).await
            }
            res => res,
        }
    }

    /// Get an auto-generated YouTube Mix from the `next` endpoint
    async fn playlist_mix(&self, playlist_id: &str) -> Result<Playlist, Error> {
        // Mixes need a seed video, which is the last 11 characters of the ID
        // (`RD<video id>` or `RDGMEM<...><video id>`). Without it, YouTube does not
        // return all mixes.
        let seed = playlist_id
            .len()
            .checked_sub(11)
            .filter(|n| *n >= 2)
            .and_then(|n| playlist_id.get(n..));
        let request_body = match seed {
            Some(video_id) => ytbody!({
                "videoId": video_id,
                "playlistId": playlist_id,
            }),
            None => ytbody!({
                "playlistId": playlist_id,
            }),
        };

        self.execute_request::<MixJson, _, _>(
            ClientType::Desktop,
            "playlist_mix",
            playlist_id,
            "next",
            &request_body,
        )
        .await
    }

    async fn playlist_browse(&self, playlist_id: &str) -> Result<Playlist, Error> {
        let request_body = ytbody!({
            "browseId": format!("VL{playlist_id}"),
        });

        self.execute_request::<PlaylistJson, _, _>(
            ClientType::Desktop,
            "playlist",
            playlist_id,
            "browse",
            &request_body,
        )
        .await
    }
}

fn json_alerts_to_err(id: &str, root: &JsonNode<'_>) -> ExtractionError {
    let alerts = root.query(ytq!(.alerts)).map(|node| {
        let (alerts, _) = node.deserialize_items_lossy::<response::Alert>();
        alerts
    });
    response::alerts_to_err(id, alerts)
}

fn yt_playlist_video_list<'a>(root: &JsonNode<'a>) -> Result<JsonNode<'a>, ExtractionError> {
    let browse = root.require(
        ytq!(.contents.twoColumnBrowseResultsRenderer),
        "two column browse results",
    )?;
    let tab = yt_first_tab(&browse).ok_or({
        ExtractionError::InvalidData(Cow::Borrowed("twoColumnBrowseResultsRenderer empty"))
    })?;
    let sections = tab.require(
        ytq!(.tabRenderer.content.sectionListRenderer.contents),
        "section list renderer",
    )?;
    let section = sections.items().into_iter().next().ok_or({
        ExtractionError::InvalidData(Cow::Borrowed("sectionListRenderer empty"))
    })?;
    let item_section = section.require(
        ytq!(.itemSectionRenderer.contents),
        "item section renderer",
    )?;
    if let Some(list) = item_section.items().into_iter().next().and_then(|item| {
        item.first_of(&[
            ytq!(.playlistVideoListRenderer.contents),
            ytq!(.richGridRenderer.contents),
        ])
    }) {
        return Ok(list);
    }
    if item_section.items().is_empty() {
        return Err(ExtractionError::InvalidData(Cow::Borrowed(
            "itemSectionRenderer empty",
        )));
    }
    Ok(item_section)
}

impl MapJsonResponse<Playlist> for PlaylistJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Playlist>, ExtractionError> {
        json.with_root(|root| {
            let contents = root.query(ytq!(.contents.twoColumnBrowseResultsRenderer));
            let header = root.query(ytq!(.header));
            if contents.is_none() || header.is_none() {
                return Err(json_alerts_to_err(ctx.id, &root));
            }

            let video_items = yt_playlist_video_list(&root)?;
            let mut mapper = response::YouTubeListMapper::<VideoItem>::new(ctx.lang);
            mapper.map_response_node(&video_items);

            let (description, thumbnails, last_update_txt) = match root.query(ytq!(.sidebar)) {
                Some(sidebar) => {
                    let sidebar_items = sidebar
                        .require(
                            ytq!(.playlistSidebarRenderer.items),
                            "playlist sidebar items",
                        )?
                        .items();
                    let primary = sidebar_items.into_iter().next().ok_or({
                        ExtractionError::InvalidData(Cow::Borrowed("no primary sidebar"))
                    })?;
                    let info = primary.require(
                        ytq!(.playlistSidebarPrimaryInfoRenderer),
                        "playlist sidebar primary info",
                    )?;

                    (
                        info.query(ytq!(.description))
                            .and_then(|node| node.deserialize::<TextComponents>().ok())
                            .filter(|d| !d.0.is_empty()),
                        info.query(ytq!(.thumbnailRenderer.playlistVideoThumbnailRenderer.thumbnail))
                            .or_else(|| {
                                info.query(
                                    ytq!(.thumbnailRenderer.playlistCustomThumbnailRenderer.thumbnail),
                                )
                            })
                            .map(|node| yt_thumbnails(&node)),
                        info.query(ytq!(.stats))
                            .map(|stats| {
                                stats
                                    .items()
                                    .into_iter()
                                    .filter_map(|item| item.text())
                                    .collect::<Vec<_>>()
                            })
                            .and_then(|mut stats| stats.try_swap_remove(2)),
                    )
                }
                None => (None, None, None),
            };

            let header: response::playlist::Header = header
                .ok_or(ExtractionError::InvalidData(Cow::Borrowed("no header")))?
                .deserialize()?;

            let (
                name,
                playlist_id,
                channel,
                n_videos_txt,
                description2,
                thumbnails2,
                last_update_txt2,
            ) = match header {
                response::playlist::Header::PlaylistHeaderRenderer(header_renderer) => {
                    let mut byline = header_renderer.byline;
                    let last_update_txt = byline
                        .try_swap_remove(1)
                        .map(|b| b.playlist_byline_renderer.text);

                    (
                        header_renderer.title,
                        header_renderer.playlist_id,
                        header_renderer
                            .owner_text
                            .and_then(|link| ChannelId::try_from(link).ok()),
                        header_renderer.num_videos_text,
                        header_renderer
                            .description_text
                            .map(|text| TextComponents(vec![TextComponent::new(text)])),
                        header_renderer
                            .playlist_header_banner
                            .map(|b| b.hero_playlist_thumbnail_renderer.thumbnail),
                        last_update_txt,
                    )
                }
                response::playlist::Header::PageHeaderRenderer(content_renderer) => {
                    let h = content_renderer.content.page_header_view_model;
                    let rows = h.metadata.content_metadata_view_model.metadata_rows;
                    let n_videos_txt = rows
                        .get(1)
                        .and_then(|r| r.metadata_parts.get(1))
                        .map(|p| p.as_str().to_owned())
                        .ok_or(ExtractionError::InvalidData("no video count".into()))?;
                    let mut channel = rows
                        .into_iter()
                        .next()
                        .and_then(|r| r.metadata_parts.into_iter().next())
                        .and_then(|p| match p {
                            response::MetadataPart::Text { .. } => None,
                            response::MetadataPart::AvatarStack { avatar_stack } => {
                                ChannelId::try_from(avatar_stack.avatar_stack_view_model.text).ok()
                            }
                        });
                    // remove "by" prefix
                    if let Some(c) = channel.as_mut() {
                        let entry = dictionary::entry(ctx.lang);
                        let n = c.name.strip_prefix(entry.chan_prefix).unwrap_or(&c.name);
                        let n = n.strip_suffix(entry.chan_suffix).unwrap_or(n);
                        c.name = n.trim().to_owned();
                    }

                    let playlist_id = h
                        .actions
                        .flexible_actions_view_model
                        .actions_rows
                        .into_iter()
                        .next()
                        .and_then(|r| r.actions.into_iter().next())
                        .and_then(|a| {
                            a.button_view_model
                                .on_tap
                                .innertube_command
                                .into_playlist_id()
                        })
                        .ok_or(ExtractionError::InvalidData("no playlist id".into()))?;
                    (
                        h.title.dynamic_text_view_model.text,
                        playlist_id,
                        channel,
                        n_videos_txt,
                        h.description.description_preview_view_model.description,
                        h.hero_image.content_preview_image_view_model.image.into(),
                        None,
                    )
                }
            };

            let n_videos = if mapper.ctoken.is_some() {
                util::parse_numeric(&n_videos_txt)
                    .map_err(|_| ExtractionError::InvalidData("no video count".into()))?
            } else {
                mapper.items.len() as u64
            };

            if playlist_id != ctx.id {
                return Err(ExtractionError::WrongResult(format!(
                    "got wrong playlist id {}, expected {}",
                    playlist_id, ctx.id
                )));
            }

            let description = description.or(description2).map(RichText::from);
            let thumbnails = thumbnails
                .or_else(|| thumbnails2.map(|t| t.into()))
                .ok_or(ExtractionError::InvalidData(Cow::Borrowed(
                    "no thumbnail found",
                )))?;
            let last_update = last_update_txt
                .as_deref()
                .or(last_update_txt2.as_deref())
                .and_then(|txt| {
                    timeago::parse_textual_date_or_warn(
                        ctx.lang,
                        ctx.utc_offset,
                        txt,
                        &mut mapper.warnings,
                    )
                    .map(OffsetDateTime::date)
                });

            Ok(MapResult {
                c: Playlist {
                    id: playlist_id,
                    name,
                    videos: Paginator::new_ext(
                        Some(n_videos),
                        mapper.items,
                        mapper.ctoken,
                        ctx.visitor_data.map(str::to_owned),
                        ContinuationEndpoint::Browse,
                        ctx.authenticated,
                    ),
                    video_count: n_videos,
                    thumbnail: thumbnails,
                    description,
                    channel,
                    last_update,
                    last_update_txt,
                    visitor_data: yt_response_visitor_data(&root)
                        .or_else(|| ctx.visitor_data.map(str::to_owned)),
                },
                warnings: mapper.warnings,
            })
        })
    }
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MixPanelVideo {
    video_id: String,
    #[serde_as(as = "Text")]
    title: String,
    #[serde(default)]
    thumbnail: response::Thumbnails,
    #[serde_as(as = "Option<Text>")]
    length_text: Option<String>,
    short_byline_text: Option<TextComponent>,
    long_byline_text: Option<TextComponent>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
enum MixPanelItem {
    PlaylistPanelVideoRenderer(Box<MixPanelVideo>),
    #[serde(other)]
    Unknown,
}

impl MapJsonResponse<Playlist> for MixJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Playlist>, ExtractionError> {
        json.with_root(|root| {
            let playlist = root
                .query(ytq!(.contents.twoColumnWatchNextResults.playlist.playlist))
                .ok_or_else(|| ExtractionError::NotFound {
                    id: ctx.id.to_owned(),
                    msg: "mix not found".into(),
                })?;
            let name = playlist
                .query(ytq!(.title))
                .and_then(|t| t.text())
                .ok_or(ExtractionError::InvalidData("no mix title".into()))?;
            let (items, mut warnings) = playlist
                .query(ytq!(.contents))
                .map(|c| c.deserialize_items_lossy::<MixPanelItem>())
                .unwrap_or_default();

            let videos = items
                .into_iter()
                .filter_map(|item| match item {
                    MixPanelItem::PlaylistPanelVideoRenderer(v) => Some(VideoItem {
                        id: v.video_id,
                        name: v.title,
                        duration: v.length_text.as_deref().and_then(util::parse_video_length),
                        thumbnail: v.thumbnail.into(),
                        channel: v
                            .long_byline_text
                            .or(v.short_byline_text)
                            .and_then(|c| ChannelTag::try_from(c).ok()),
                        publish_date: None,
                        publish_date_txt: None,
                        view_count: None,
                        is_live: false,
                        is_short: false,
                        is_upcoming: false,
                        short_description: None,
                    }),
                    MixPanelItem::Unknown => {
                        warnings.push("unknown mix item".to_owned());
                        None
                    }
                })
                .collect::<Vec<_>>();
            let n_videos = videos.len() as u64;

            Ok(MapResult {
                c: Playlist {
                    id: ctx.id.to_owned(),
                    name,
                    // Mixes are infinite, only the first batch is returned
                    videos: Paginator::new_ext(
                        Some(n_videos),
                        videos,
                        None,
                        ctx.visitor_data.map(str::to_owned),
                        ContinuationEndpoint::Browse,
                        ctx.authenticated,
                    ),
                    video_count: n_videos,
                    thumbnail: Vec::new(),
                    description: None,
                    channel: None,
                    last_update: None,
                    last_update_txt: None,
                    visitor_data: yt_response_visitor_data(&root)
                        .or_else(|| ctx.visitor_data.map(str::to_owned)),
                },
                warnings,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use path_macro::path;
    use rstest::rstest;

    use crate::util::tests::TESTFILES;

    use super::{MapJsonResponse, *};

    #[rstest]
    #[case::short("short", "RDCLAK5uy_kFQXdnqMaQCVx2wpUM4ZfbsGCDibZtkJk")]
    #[case::long("long", "PL5dDx681T4bR7ZF1IuWzOv1omlRbE7PiJ")]
    #[case::nomusic("nomusic", "PL1J-6JOckZtE_P9Xx8D3b2O6w0idhuKBe")]
    #[case::live("live", "UULVvqRdlKsE5Q8mf8YXbdIJLw")]
    #[case::pageheader("20241011_pageheader", "PLT2w2oBf1TZKyvY_M6JsASs73m-wjLzH5")]
    #[case::cmdexecutor("20250316_cmdexecutor", "PLbZIPy20-1pN7mqjckepWF78ndb6ci_qi")]
    #[case::lockup("20260920_lockup", "PLRCwNR3H9ZNKQTzdmu3qlwei9_74VIwg7")]
    fn map_playlist_data(#[case] name: &str, #[case] id: &str) {
        let json_path = path!(*TESTFILES / "playlist" / format!("playlist_{name}.json"));
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Playlist> =
            PlaylistJson::map_json_response(&json, &MapRespCtx::test(id)).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        insta::assert_ron_snapshot!(format!("map_playlist_data_{name}"), map_res.c, {
            ".last_update" => "[date]",
            ".videos.items[].publish_date" => "[date]",
        });
    }

    #[test]
    fn map_playlist_mix() {
        let json_path = path!(*TESTFILES / "playlist" / "mix.json");
        let json = JsonDoc::new(fs::read_to_string(json_path).unwrap());
        let map_res: MapResult<Playlist> =
            MixJson::map_json_response(&json, &MapRespCtx::test("RDXdiJkzQwKd8")).unwrap();

        assert!(
            map_res.warnings.is_empty(),
            "deserialization/mapping warnings: {:?}",
            map_res.warnings
        );
        assert_eq!(map_res.c.videos.items.len(), 25);
        assert!(map_res.c.name.starts_with("Mix"));
        assert!(map_res
            .c
            .videos
            .items
            .iter()
            .all(|v| v.duration.is_some() && v.channel.as_ref().is_some_and(|c| !c.id.is_empty())));
        insta::assert_ron_snapshot!(map_res.c, {
            ".visitor_data" => "[visitor_data]",
        });
    }

    #[rstest]
    #[case("RDXdiJkzQwKd8", true)]
    #[case("RDGMEMCMFH2exzjBeE_zAHHJOdxgVMmj3S8FL7whI", true)]
    #[case("RDCLAK5uy_kFQXdnqMaQCVx2wpUM4ZfbsGCDibZtkJk", false)]
    #[case("PL5dDx681T4bR7ZF1IuWzOv1omlRbE7PiJ", false)]
    fn mix_id(#[case] id: &str, #[case] expect: bool) {
        assert_eq!(is_mix_id(id), expect);
    }
}
