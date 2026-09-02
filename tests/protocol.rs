use agentx::protocol::Image;

#[tokio::test]
async fn local_images_become_persistable_data_urls() {
  let temp = tempfile::tempdir().unwrap();
  let path = temp.path().join("sample.png");
  tokio::fs::write(&path, [1_u8, 2, 3]).await.unwrap();

  let image = Image::from_path(&path).await.unwrap();
  assert_eq!(image.url, "data:image/png;base64,AQID");
  assert_eq!(image.detail, None);
}
