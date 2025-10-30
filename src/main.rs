use anyhow::{Result, anyhow};
use reqwest::Client;

struct CrateInfo {
    name: String,
    version: String,
}

struct CrateScore {
    name: String,
    version: String,
    repository: Option<String>,
    security_score: Option<f64>,
}

fn get_dependencies() -> Result<Vec<CrateInfo>> {
    let output = std::process::Command::new("sh")
        .args(["-c", "cargo tree --prefix none | sort -u"])
        .output()
        .map_err(|e| anyhow!("Failed to run cargo tree: {}", e))?;

    if !output.status.success() {
        return Err(anyhow!("cargo tree with sort failed"));
    }

    let dependencies = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.split_whitespace().collect::<Vec<&str>>())
        .filter(|parts| parts.len() == 2)
        .map(|parts| CrateInfo {
            name: parts[0].to_string(),
            version: parts[1].to_string(),
        })
        .collect();

    Ok(dependencies)
}

async fn fetch_crate_repo_url(client: &Client, crate_name: &str) -> Result<Option<String>> {
    let url = format!("https://crates.io/api/v1/crates/{}", crate_name);

    let response = client
        .get(&url)
        .header("User-Agent", "cargo-scorecard/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch crate repo url for {}: {}", crate_name, e))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "API request failed for {}: {}",
            crate_name,
            response.status()
        ));
    }

    let json: serde_json::Value = response
        .json()
        .await
        .map_err(|e| anyhow!("Failed to parse JSON for {}: {}", crate_name, e))?;

    let repository = json["crate"]["repository"].as_str().map(|s| s.to_string());

    Ok(repository)
}

async fn fetch_security_score(client: &reqwest::Client, repo_url: &str) -> Result<Option<f64>> {
    let url = format!(
        "https://api.securityscorecards.dev/projects/{}",
        repo_url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
    );

    let response = client
        .get(&url)
        .header("accept", "application/json")
        .header("User-Agent", "cargo-scorecard/0.1.0")
        .send()
        .await
        .map_err(|e| anyhow!("Failed to fetch security score for {}: {}", repo_url, e))?;

    if !response.status().is_success() {
        return Err(anyhow!(
            "Security scorecard API request failed for {}: {}",
            repo_url,
            response.status()
        ));
    }

    let json: serde_json::Value = response.json().await.map_err(|e| {
        anyhow!(
            "Failed to parse security score JSON for {}: {}",
            repo_url,
            e
        )
    })?;

    Ok(json["score"].as_f64())
}

async fn fetch_crate_score(client: &Client, crate_info: &CrateInfo) -> Result<CrateScore> {
    // First, get the repository URL
    let repository = fetch_crate_repo_url(client, &crate_info.name).await?;

    // If we have a repository URL, fetch the security score
    let security_score = if let Some(ref repo_url) = repository {
        fetch_security_score(client, repo_url).await.unwrap_or(None)
    } else {
        None
    };

    Ok(CrateScore {
        name: crate_info.name.clone(),
        version: crate_info.version.clone(),
        repository,
        security_score,
    })
}

fn main() -> Result<()> {
    // Step 1: Get basic dependencies (fast, local operation)
    println!("Parsing dependencies...");
    let crates = get_dependencies()?;

    println!("Found {} dependencies", crates.len());

    // Step 2: Create HTTP client for API requests
    let client = reqwest::Client::new();

    println!("Fetching repository URLs and security scores...");

    // Step 3: Fetch all crate scores concurrently using minimal Tokio runtime
    let results = tokio::runtime::Runtime::new()?.block_on(futures::future::join_all(
        crates
            .iter()
            .map(|crate_info| fetch_crate_score(&client, crate_info)),
    ));

    // Step 5: Display results in markdown table format
    println!("\n## Cargo Scorecard Results\n");
    println!("| Crate Name | Version | Repository URL | Security Score |");
    println!("| --- | --- | --- | --- |");

    for crate_score in results.into_iter().filter_map(Result::ok) {
        let repo_url = match &crate_score.repository {
            Some(repo) => repo.clone(),
            None => "No repository information".to_string(),
        };
        let score = match crate_score.security_score {
            Some(score) => format!("{:.1}", score),
            None => "Not available".to_string(),
        };
        println!(
            "| {} | {} | {} | {} |",
            crate_score.name, crate_score.version, repo_url, score
        );
    }

    Ok(())
}
