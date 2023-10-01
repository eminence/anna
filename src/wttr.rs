use anyhow::Context;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(JsonSchema, Serialize, Deserialize, Debug)]
pub struct WeatherInput {
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub country: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WeatherOutput {
    pub current_condition: Vec<CurrentCondition>,
    pub nearest_area: Vec<Area>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct CurrentCondition {
    #[serde(rename = "temp_C")]
    pub temp_c: String,
    #[serde(rename = "temp_F")]
    pub temp_f: String,
    #[serde(rename = "weatherDesc")]
    pub weather_desc: Vec<WeatherDesc>,
    pub humidity: String,
    #[serde(rename = "windspeedKmph")]
    pub windspeed_kmph: String,
    #[serde(rename = "winddir16Point")]
    pub winddir16_point: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WeatherDesc {
    pub value: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct Area {
    #[serde(rename = "areaName")]
    pub area_name: Vec<WeatherDesc>,
    pub country: Vec<WeatherDesc>,
    pub region: Vec<WeatherDesc>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WeatherOutputForChat {
    pub temp_c: String,
    pub temp_f: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    pub humidity: String,
    pub windspeed_kmph: String,
    pub wind_direction: String,
}

pub async fn get_weather(input: &WeatherInput) -> anyhow::Result<WeatherOutputForChat> {
    dbg!(&input);
    let fields = [
        input.city.as_str(),
        &input.state.as_str(),
        input.country.as_str(),
    ];

    let url = format!("https://wttr.in/{}?format=j1", fields.join("+")).replace(" ", "%20");
    dbg!(&url);

    let req = reqwest::get(&url).await?;
    let mut resp = req.json::<WeatherOutput>().await?;
    dbg!(&resp);

    let mut current = resp
        .current_condition
        .pop()
        .context("No current condition")?;

    let output = WeatherOutputForChat {
        temp_c: current.temp_c,
        temp_f: current.temp_f,
        humidity: current.humidity,
        windspeed_kmph: current.windspeed_kmph,
        wind_direction: current.winddir16_point,
        description: current.weather_desc.pop().map(|desc| desc.value),
        location: resp.nearest_area.pop().map(|mut area| {
            format!(
                "{}, {}",
                area.area_name.pop().unwrap().value,
                area.region.pop().unwrap().value
            )
        }),
    };
    Ok(output)
}

#[tokio::test]
async fn test_get_weather() {
    let input = WeatherInput {
        city: "Duckman".to_string(),
        state: "".to_string(),
        country: "".to_string(),
    };

    let weather = get_weather(&input).await.unwrap();
    let json = serde_json::to_string(&weather).unwrap();
    println!("{}", json);
}
