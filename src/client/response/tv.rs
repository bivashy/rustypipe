//! Response models for the YouTube TV client (home feed, history, subscriptions,
//! saved playlists, liked videos, watch later)
//!
//! All TV browse responses are built from "tiles" (`tileRenderer`) which are either
//! nested in shelves of a section list or listed directly in a grid / playlist video list.
//! This module contains one tile mapper and a generic list extractor for all of them.
//!
//! Note that channel IDs are not available for most video tiles from the TV client.
//! In these cases `channel.id` is empty.

use serde::Deserialize;
use serde_with::{serde_as, VecSkipError};

use crate::client::{MapJsonResponse, MapRespCtx};
use crate::error::ExtractionError;
use crate::json::{yt_continuation, ytq, JsonDoc, JsonNode};
use crate::model::{
    paginator::{ContinuationEndpoint, Paginator},
    traits::FromYtItem,
    ChannelItem, ChannelTag, PlaylistItem, Verification, VideoItem, YouTubeItem,
};
use crate::serializer::{text::Text, MapResult};
use crate::util::{self, timeago};

#[cfg(feature = "userdata")]
use crate::model::{ChannelId, HistoryItem, Playlist};

use super::Thumbnails;

/// Marker type for mapping TV client list responses
/// (home feed, history, subscriptions, ... and their continuations)
#[derive(Debug)]
pub(crate) struct TvFeedJson;

/// Marker type for mapping the TV client playlist page (liked videos, watch later)
#[cfg(feature = "userdata")]
#[derive(Debug)]
pub(crate) struct TvPlaylistJson;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TvSection {
    ShelfRenderer(ShelfRenderer),
    TileRenderer(Box<TileRenderer>),
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShelfRenderer {
    pub content: ShelfContent,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShelfContent {
    pub horizontal_list_renderer: Option<HorizontalListRenderer>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HorizontalListRenderer {
    pub items: MapResult<Vec<TvTile>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TvTile {
    pub tile_renderer: TileRenderer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) enum TileContentType {
    #[serde(rename = "TILE_CONTENT_TYPE_VIDEO")]
    Video,
    #[serde(rename = "TILE_CONTENT_TYPE_SHORTS")]
    Shorts,
    #[serde(rename = "TILE_CONTENT_TYPE_PLAYLIST")]
    Playlist,
    #[serde(rename = "TILE_CONTENT_TYPE_CHANNEL")]
    Channel,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TileRenderer {
    pub content_id: String,
    pub content_type: TileContentType,
    pub header: TileHeader,
    pub metadata: Option<TileMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TileHeader {
    pub tile_header_renderer: TileHeaderRenderer,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TileHeaderRenderer {
    #[serde(default)]
    pub thumbnail: Thumbnails,
    #[serde(default)]
    #[serde_as(as = "VecSkipError<_>")]
    pub thumbnail_overlays: Vec<TileOverlay>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum TileOverlay {
    ThumbnailOverlayTimeStatusRenderer(TimeStatusOverlay),
    /// Shorts tiles carry their title in the thumbnail overlay
    TileMetadataRenderer(TileMetadataRenderer),
    #[serde(other)]
    Unknown,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TimeStatusOverlay {
    #[serde_as(as = "Text")]
    pub text: String,
    #[serde(default)]
    pub style: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TileMetadata {
    pub tile_metadata_renderer: TileMetadataRenderer,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TileMetadataRenderer {
    #[serde_as(as = "Option<Text>")]
    pub title: Option<String>,
    #[serde(default)]
    pub lines: Vec<TileLine>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TileLine {
    pub line_renderer: LineRenderer,
}

#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineRenderer {
    #[serde(default)]
    #[serde_as(as = "VecSkipError<_>")]
    pub items: Vec<LineItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineItem {
    pub line_item_renderer: LineItemRenderer,
}

/// Line item. Badge items do not have a text.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineItemRenderer {
    pub text: Option<LineText>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum LineText {
    Simple {
        #[serde(rename = "simpleText")]
        simple_text: String,
    },
    Runs {
        runs: Vec<LineRun>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineRun {
    pub text: String,
    pub navigation_endpoint: Option<LineNavEndpoint>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineNavEndpoint {
    pub browse_endpoint: Option<LineBrowseEndpoint>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LineBrowseEndpoint {
    pub browse_id: String,
}

/// Text part of a metadata line with an optional browse ID (channel ID)
#[derive(Debug)]
struct LinePart {
    text: String,
    browse_id: Option<String>,
}

impl LineText {
    fn into_part(self) -> LinePart {
        match self {
            LineText::Simple { simple_text } => LinePart {
                text: simple_text,
                browse_id: None,
            },
            LineText::Runs { runs } => LinePart {
                browse_id: runs.iter().find_map(|r| {
                    r.navigation_endpoint
                        .as_ref()
                        .and_then(|e| e.browse_endpoint.as_ref())
                        .map(|b| b.browse_id.clone())
                }),
                text: runs.into_iter().map(|r| r.text).collect(),
            },
        }
    }
}

impl TileLine {
    /// Text parts of the line without separators and badges
    fn into_parts(self) -> Vec<LinePart> {
        self.line_renderer
            .items
            .into_iter()
            .filter_map(|i| i.line_item_renderer.text)
            .map(LineText::into_part)
            .filter(|p| {
                let t = p.text.trim();
                !t.is_empty() && t != "•"
            })
            .collect()
    }
}

fn has_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

/// Fix protocol-relative thumbnail URLs (`//yt3.googleusercontent.com/...`)
fn map_thumbnails(thumbnails: Thumbnails) -> Vec<crate::model::Thumbnail> {
    let mut thumbnails: Vec<crate::model::Thumbnail> = thumbnails.into();
    for t in &mut thumbnails {
        if t.url.starts_with("//") {
            t.url = format!("https:{}", t.url);
        }
    }
    thumbnails
}

fn map_tile(
    tile: TileRenderer,
    ctx: &MapRespCtx<'_>,
    warnings: &mut Vec<String>,
) -> Option<YouTubeItem> {
    let header = tile.header.tile_header_renderer;
    let md = tile.metadata.map(|m| m.tile_metadata_renderer);

    let time_overlay = header.thumbnail_overlays.iter().find_map(|o| match o {
        TileOverlay::ThumbnailOverlayTimeStatusRenderer(t) => Some(t),
        _ => None,
    });
    let title = md.as_ref().and_then(|m| m.title.clone()).or_else(|| {
        header.thumbnail_overlays.iter().find_map(|o| match o {
            TileOverlay::TileMetadataRenderer(m) => m.title.clone(),
            _ => None,
        })
    });
    let Some(name) = title else {
        warnings.push(format!("tile {}: no title", tile.content_id));
        return None;
    };

    // The lines of a tile are heterogeneous (channel, views, date, `Song`, `Public`, artist
    // lists, ...). Anything that is not recognized is ignored.
    let mut lines = md
        .map(|m| m.lines)
        .unwrap_or_default()
        .into_iter()
        .map(TileLine::into_parts);
    let line0 = lines.next().unwrap_or_default();
    let rest = lines.flatten().collect::<Vec<_>>();

    match tile.content_type {
        TileContentType::Video => {
            let is_live = time_overlay.is_some_and(|t| t.style == "LIVE");
            let channel = line0.into_iter().next().map(|p| ChannelTag {
                // Channel IDs are only present in some TV responses (e.g. history)
                id: p.browse_id.unwrap_or_default(),
                name: p.text.trim().to_owned(),
                avatar: Vec::new(),
                verification: Verification::None,
                subscriber_count: None,
            });

            let mut view_count = None;
            let mut publish_date = None;
            let mut publish_date_txt = None;
            // Views and publish date cannot be told apart by parsing alone (`160M views`
            // is also a valid timeago text). Videos with a date always have a view count
            // before it (`No views` for zero views), so the first text with digits is the
            // view count unless it is preceded by `No views`.
            let mut n_numeric = 0;
            for part in rest {
                let t = part.text.trim();
                if t.eq_ignore_ascii_case("no views") {
                    view_count = Some(0);
                    continue;
                }
                if !has_digit(t) {
                    continue;
                }
                n_numeric += 1;
                if view_count.is_none() && n_numeric == 1 {
                    view_count = util::parse_large_numstr::<u64>(t, ctx.lang);
                } else if publish_date.is_none() {
                    publish_date = timeago::parse_timeago_dt(ctx.lang, t);
                    if publish_date.is_some() {
                        publish_date_txt = Some(t.to_owned());
                    }
                }
            }

            Some(YouTubeItem::Video(VideoItem {
                id: tile.content_id,
                name,
                duration: if is_live {
                    None
                } else {
                    time_overlay.and_then(|t| util::parse_video_length(&t.text))
                },
                thumbnail: map_thumbnails(header.thumbnail),
                channel,
                publish_date,
                publish_date_txt,
                view_count,
                is_live,
                is_short: false,
                is_upcoming: false,
                short_description: None,
            }))
        }
        TileContentType::Shorts => Some(YouTubeItem::Video(VideoItem {
            id: tile.content_id,
            name,
            duration: None,
            thumbnail: map_thumbnails(header.thumbnail),
            channel: None,
            publish_date: None,
            publish_date_txt: None,
            view_count: None,
            is_live: false,
            is_short: true,
            is_upcoming: false,
            short_description: None,
        })),
        TileContentType::Playlist => Some(YouTubeItem::Playlist(PlaylistItem {
            id: tile.content_id,
            name,
            thumbnail: map_thumbnails(header.thumbnail),
            // Only set if the line contains a link to a channel (not for `Public`/`Private` or mixes)
            channel: line0.into_iter().next().and_then(|p| {
                p.browse_id.map(|id| ChannelTag {
                    id,
                    name: p.text.trim().to_owned(),
                    avatar: Vec::new(),
                    verification: Verification::None,
                    subscriber_count: None,
                })
            }),
            // Mixes have the text `Mix` instead of a video count
            video_count: time_overlay
                .filter(|t| has_digit(&t.text))
                .and_then(|t| util::parse_numeric::<u64>(&t.text).ok()),
        })),
        TileContentType::Channel => {
            let mut parts = line0.into_iter().chain(rest).collect::<Vec<_>>();
            let handle = parts
                .iter()
                .position(|p| p.text.trim().starts_with('@'))
                .map(|i| parts.remove(i).text.trim().to_owned());
            let subscriber_count = parts
                .iter()
                .filter(|p| has_digit(&p.text))
                .find_map(|p| util::parse_large_numstr::<u64>(p.text.trim(), ctx.lang));
            Some(YouTubeItem::Channel(ChannelItem {
                id: tile.content_id,
                name,
                handle,
                avatar: map_thumbnails(header.thumbnail),
                verification: Verification::None,
                subscriber_count,
                short_description: String::new(),
            }))
        }
        TileContentType::Unknown => {
            warnings.push(format!("tile {}: unknown content type", tile.content_id));
            None
        }
    }
}

/// Find the list node (section list, grid or playlist video list) of
/// a TV initial or continuation response
fn tv_list_node<'a>(root: &JsonNode<'a>) -> Option<JsonNode<'a>> {
    root.first_of(&[
        ytq!(.contents.tvBrowseRenderer.content.tvSurfaceContentRenderer.content.sectionListRenderer),
        ytq!(.contents.tvBrowseRenderer.content.tvSurfaceContentRenderer.content.gridRenderer),
        ytq!(.contents.tvBrowseRenderer.content.tvSurfaceContentRenderer.content.twoColumnRenderer.rightColumn.playlistVideoListRenderer),
        // Tab 0 is the list of all channels/videos, other tabs are filtered by channel
        ytq!(.contents.tvBrowseRenderer.content.tvSecondaryNavRenderer.sections[0].tvSecondaryNavSectionRenderer.tabs[0].tabRenderer.content.tvSurfaceContentRenderer.content.sectionListRenderer),
        ytq!(.continuationContents.sectionListContinuation),
        ytq!(.continuationContents.gridContinuation),
        ytq!(.continuationContents.tvSurfaceContentContinuation.content.gridRenderer),
        ytq!(.continuationContents.playlistVideoListContinuation),
    ])
}

fn map_tv_list(
    root: &JsonNode<'_>,
    ctx: &MapRespCtx<'_>,
) -> Result<MapResult<Paginator<YouTubeItem>>, ExtractionError> {
    let list = tv_list_node(root).ok_or_else(|| {
        ExtractionError::InvalidData("no TV list content (not logged in?)".into())
    })?;

    let mut warnings = Vec::new();
    let mut items = Vec::new();

    if let Some(contents) = list
        .query(ytq!(.contents))
        .or_else(|| list.query(ytq!(.items)))
    {
        let (sections, mut w) = contents.deserialize_items_lossy::<TvSection>();
        warnings.append(&mut w);
        for section in sections {
            match section {
                TvSection::ShelfRenderer(shelf) => {
                    let Some(list) = shelf.content.horizontal_list_renderer else {
                        warnings.push("shelf without horizontalListRenderer".to_owned());
                        continue;
                    };
                    let mut tiles = list.items;
                    warnings.append(&mut tiles.warnings);
                    for tile in tiles.c {
                        if let Some(item) = map_tile(tile.tile_renderer, ctx, &mut warnings) {
                            items.push(item);
                        }
                    }
                }
                TvSection::TileRenderer(tile) => {
                    if let Some(item) = map_tile(*tile, ctx, &mut warnings) {
                        items.push(item);
                    }
                }
                TvSection::Unknown => warnings.push("unknown TV list section".to_owned()),
            }
        }
    }

    let ctoken = list
        .query(ytq!(.continuations[0]))
        .and_then(|c| yt_continuation(&c));

    Ok(MapResult {
        c: Paginator::new_ext(
            None,
            items,
            ctoken,
            ctx.visitor_data.map(str::to_owned),
            ContinuationEndpoint::TvBrowse,
            true,
        ),
        warnings,
    })
}

impl<T: FromYtItem> MapJsonResponse<Paginator<T>> for TvFeedJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Paginator<T>>, ExtractionError> {
        let res = json.with_root(|root| map_tv_list(&root, ctx))?;
        Ok(MapResult {
            c: convert_paginator(res.c),
            warnings: res.warnings,
        })
    }
}

fn convert_paginator<T: FromYtItem>(p: Paginator<YouTubeItem>) -> Paginator<T> {
    Paginator {
        count: p.count,
        items: p.items.into_iter().filter_map(T::from_yt_item).collect(),
        ctoken: p.ctoken,
        visitor_data: p.visitor_data,
        endpoint: p.endpoint,
        authenticated: p.authenticated,
    }
}

/// The TV client does not return playback dates for history items
#[cfg(feature = "userdata")]
impl MapJsonResponse<Paginator<HistoryItem<VideoItem>>> for TvFeedJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Paginator<HistoryItem<VideoItem>>>, ExtractionError> {
        let res = json.with_root(|root| map_tv_list(&root, ctx))?;
        let p = res.c;
        Ok(MapResult {
            c: Paginator {
                count: p.count,
                items: p
                    .items
                    .into_iter()
                    .filter_map(VideoItem::from_yt_item)
                    .map(|item| HistoryItem {
                        item,
                        playback_date: None,
                        playback_date_txt: None,
                    })
                    .collect(),
                ctoken: p.ctoken,
                visitor_data: p.visitor_data,
                endpoint: p.endpoint,
                authenticated: p.authenticated,
            },
            warnings: res.warnings,
        })
    }
}

#[cfg(feature = "userdata")]
#[serde_as]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityMetadata {
    #[serde_as(as = "Text")]
    title: String,
    #[serde(default)]
    bylines: Vec<TileLine>,
}

#[cfg(feature = "userdata")]
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntityMetadataWrap {
    entity_metadata_renderer: EntityMetadata,
}

#[cfg(feature = "userdata")]
impl MapJsonResponse<Playlist> for TvPlaylistJson {
    fn map_json_response(
        json: &JsonDoc,
        ctx: &MapRespCtx<'_>,
    ) -> Result<MapResult<Playlist>, ExtractionError> {
        json.with_root(|root| {
            let header = root
                .query(ytq!(.contents.tvBrowseRenderer.content.tvSurfaceContentRenderer.content.twoColumnRenderer.leftColumn))
                .ok_or_else(|| ExtractionError::InvalidData("no playlist header".into()))?
                .deserialize::<EntityMetadataWrap>()?
                .entity_metadata_renderer;
            let mut res = map_tv_list(&root, ctx)?;
            let videos = convert_paginator::<VideoItem>(res.c);

            // Byline: [owner, `N videos`, views]
            let mut parts = header
                .bylines
                .into_iter()
                .flat_map(TileLine::into_parts)
                .collect::<Vec<_>>()
                .into_iter();
            let owner = parts.next();
            let video_count = parts
                .find(|p| has_digit(&p.text))
                .and_then(|p| util::parse_numeric::<u64>(&p.text).ok());

            if video_count.is_none() {
                res.warnings.push("could not find playlist video count".to_owned());
            }

            Ok(MapResult {
                c: Playlist {
                    id: ctx.id.to_owned(),
                    name: header.title,
                    video_count: video_count.unwrap_or_default(),
                    videos,
                    thumbnail: Vec::new(),
                    description: None,
                    channel: owner.map(|p| ChannelId {
                        id: p.browse_id.unwrap_or_default(),
                        name: p.text.trim().to_owned(),
                    }),
                    last_update: None,
                    last_update_txt: None,
                    visitor_data: ctx.visitor_data.map(str::to_owned),
                },
                warnings: res.warnings,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile(lines: &str) -> TileRenderer {
        serde_json::from_str(&format!(
            r#"{{
                "contentId": "abc", "contentType": "TILE_CONTENT_TYPE_VIDEO",
                "header": {{"tileHeaderRenderer": {{"thumbnail": {{"thumbnails": []}},
                    "thumbnailOverlays": [{{"thumbnailOverlayTimeStatusRenderer": {{"text": {{"simpleText": "3:39"}}, "style": "DEFAULT"}}}}]}}}},
                "metadata": {{"tileMetadataRenderer": {{"title": {{"simpleText": "T"}}, "lines": {lines}}}}}
            }}"#
        ))
        .unwrap()
    }

    fn line(items: &[&str]) -> String {
        let items = items
            .iter()
            .map(|t| format!(r#"{{"lineItemRenderer": {{"text": {{"simpleText": "{t}"}}}}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        format!(r#"{{"lineRenderer": {{"items": [{items}]}}}}"#)
    }

    #[rstest::rstest]
    #[case::song(&[&["Artist"][..], &["Song", "•", "172K views", "•", "1 month ago"][..]], Some(172_000), Some("1 month ago"))]
    #[case::no_views(&[&["Artist"][..], &["No views", "•", "5 days ago"][..]], Some(0), Some("5 days ago"))]
    #[case::views_only(&[&["Artist"][..], &["160M views"][..]], Some(160_000_000), None)]
    #[case::unknown(&[&["Artist"][..], &["Song", "Public"][..]], None, None)]
    fn map_video_lines(
        #[case] lines: &[&[&str]],
        #[case] views: Option<u64>,
        #[case] date_txt: Option<&str>,
    ) {
        let lines = format!(
            "[{}]",
            lines.iter().map(|l| line(l)).collect::<Vec<_>>().join(",")
        );
        let mut warnings = Vec::new();
        let item = map_tile(tile(&lines), &MapRespCtx::test(""), &mut warnings).unwrap();
        let YouTubeItem::Video(v) = item else {
            panic!("not a video")
        };
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(v.channel.unwrap().name, "Artist");
        assert_eq!(v.view_count, views);
        assert_eq!(v.publish_date_txt.as_deref(), date_txt);
        assert_eq!(v.publish_date.is_some(), date_txt.is_some());
        assert_eq!(v.duration, Some(219));
    }
}
