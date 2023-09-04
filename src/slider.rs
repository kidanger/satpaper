use std::sync::Arc;

use anyhow::Result;
use image::*;
use serde::Deserialize;
use ureq::Agent;

use super::{
    Config,
    OUTPUT_NAME
};

const SLIDER_BASE_URL: &str = "https://rammb-slider.cira.colostate.edu";
const SLIDER_SECTOR: &str = "full_disk";
const SLIDER_PRODUCT: &str = "geocolor";

pub fn composite_latest_image(config: &Arc<Config>) -> Result<()> {
    composite(
        config,
        download(config)?
    )
}

fn download(config: &Arc<Config>) -> Result<DynamicImage> {
    let image_size = config.satellite.image_size() as u32;
    let tile_count = config.satellite.tile_count();
    let tile_size = config.satellite.tile_size();

    let config = config.clone();
    let agent = Agent::new();

    let time = Time::fetch(&config)?;
    let (year, month, day) = Date::fetch(&config)?.split();

    let mut stitched = RgbaImage::new(
        image_size,
        image_size
    );

    log::info!("Downloading tiles...");

    let tiles: Vec<_> = (0..tile_count)
        .flat_map(|x| {
            (0..tile_count)
                .map(move |y| (x, y))
        })
        .map(|(x, y)| {
            let url = format!(
                "{SLIDER_BASE_URL}/data/imagery/{year}/{month}/{day}/{}---{SLIDER_SECTOR}/{SLIDER_PRODUCT}/{}/{:02}/{x:03}_{y:03}.png",
                config.satellite.id(),
                time.as_int(),
                config.satellite.max_zoom()
            );

            let agent = agent.clone();

            let bytes_handle = std::thread::spawn(move || -> Result<_> {
                use std::io::Read;

                log::debug!("Scraping thread spawned for tile at ({x}, {y}.)");

                let resp = agent
                    .get(&url)
                    .call()?;

                let len: usize = resp.header("Content-Length")
                    .expect("Response header should have Content-Length")
                    .parse()?;

                let mut buf = Vec::with_capacity(len);

                resp
                    .into_reader()
                    .take(10_000_000)
                    .read_to_end(&mut buf)?;

                log::debug!(
                    "Finished scraping tile at ({x}, {y}). Size: {}mB",
                    len as f32 / 1024.0 / 1024.0
                );

                Ok(buf)
            });

            (x, y, bytes_handle)
        })
        .collect();
    
    log::info!("Stitching tiles...");

    for (x, y, fut) in tiles {
        let tile = fut
            .join()
            .unwrap()?;

        let tile = image::load_from_memory_with_format(
            tile.as_bytes(),
            ImageFormat::Png
        )?;

        imageops::overlay(
            &mut stitched,
            &tile,
            (y * tile_size) as i64,
            (x * tile_size) as i64
        );
    }

    Ok(stitched.into())
}

fn composite(config: &Arc<Config>, image: DynamicImage) -> Result<()> {
    use std::cmp::Ordering::*;
    use image::imageops::FilterType;

    log::info!("Compositing...");

    let smaller_dim = match config.resolution_x.cmp(&config.resolution_y) {
        Less => config.resolution_x,
        Equal => config.resolution_x,
        Greater => config.resolution_y,
    };

    let disk_dim = smaller_dim as f32 * (config.disk_size as f32 / 100.0);
    let disk_dim = disk_dim.floor() as u32;

    log::info!("Resizing source image...");

    let source = imageops::resize(
        &image,
        disk_dim,
        disk_dim,
        FilterType::Lanczos3
    );

    log::info!("Generating destination image...");

    let mut destination = ImageBuffer::new(
        config.resolution_x,
        config.resolution_y
    );

    for (_, _, pixel) in destination.enumerate_pixels_mut() {
        *pixel = Rgba([u8::MIN, u8::MIN, u8::MIN, u8::MAX])
    }

    log::info!("Compositing source into destination...");

    imageops::overlay(
        &mut destination,
        &source,
        ((config.resolution_x - disk_dim) / 2) as i64,
        ((config.resolution_y - disk_dim) / 2) as i64
    );

    log::info!("Compositing complete.");

    destination.save(
        config.target_path.join(OUTPUT_NAME)
    )?;

    log::info!("Output saved.");

    Ok(())
}

pub fn fetch_latest_timestamp(config: &Config) -> Result<u64> {
    Ok(Time::fetch(config)?.as_int())   
}

#[derive(Debug, Deserialize)]
struct Time {
    #[serde(rename = "timestamps_int")]
    timestamps: Vec<u64>
}

impl Time {
    pub fn fetch(config: &Config) -> Result<Self> {
        let url = format!(
            "{SLIDER_BASE_URL}/data/json/{}/{SLIDER_SECTOR}/{SLIDER_PRODUCT}/latest_times.json",
            config.satellite.id()
        );
        
        let json = ureq::get(&url)
            .call()?
            .into_string()?;

        let mut new: Self = serde_json::from_str(&json)?;

        new.timestamps.drain(1..);
        new.timestamps.shrink_to_fit();

        Ok(new)
    }

    pub fn as_int(&self) -> u64 {
        *self
            .timestamps
            .first()
            .expect("At least one timestamp should exist")
    }
}

#[derive(Debug, Deserialize)]
struct Date {
    #[serde(rename = "dates_int")]
    dates: Vec<u64>
}

impl Date {
    pub fn fetch(config: &Config) -> Result<Self> {
        let url = format!(
            "{SLIDER_BASE_URL}/data/json/{}/{SLIDER_SECTOR}/{SLIDER_PRODUCT}/available_dates.json",
            config.satellite.id()
        );

        let json = ureq::get(&url)
            .call()?
            .into_string()?;

        let mut new: Self= serde_json::from_str(&json)?;

        new.dates.drain(1..);
        new.dates.shrink_to_fit();

        Ok(new)
    }

    pub fn as_int(&self) -> u64 {
        *self
            .dates
            .first()
            .expect("At least one timestamp should exist")
    }

    pub fn split(&self) -> (String, String, String) {
        let str = format!(
            "{}", 
            self.as_int()
        );

        (
            str[0..=3].to_owned(),
            str[4..=5].to_owned(),
            str[6..=7].to_owned()
        )
    }
}