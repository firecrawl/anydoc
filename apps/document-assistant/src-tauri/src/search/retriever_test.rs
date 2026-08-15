use super::{PageIndexEntry, SearchIndex};

#[test]
fn retrieves_the_page_containing_a_chinese_business_term() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let index = SearchIndex::open(&directory.path().join("search.db")).expect("index opens");
    index
        .index_document_pages(
            "doc-1",
            &[PageIndexEntry::text(1, "普通介绍"), PageIndexEntry::text(2, "西南区域转化率下降")],
        )
        .expect("pages index");

    let results = index.retrieve_context("doc-1", "西南转化率", 5).expect("context retrieves");
    assert_eq!(results[0].page_number, 2);
}

#[test]
fn never_returns_pages_from_another_document() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let index = SearchIndex::open(&directory.path().join("search.db")).expect("index opens");
    index.index_document_pages("doc-a", &[PageIndexEntry::text(1, "收入增长")]).expect("a indexes");
    index.index_document_pages("doc-b", &[PageIndexEntry::text(8, "收入下降")]).expect("b indexes");

    let results = index.retrieve_context("doc-a", "收入", 5).expect("context retrieves");
    assert!(results.iter().all(|page| page.document_id == "doc-a"));
}

#[test]
fn adds_at_most_one_neighbor_on_each_side() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let index = SearchIndex::open(&directory.path().join("search.db")).expect("index opens");
    index
        .index_document_pages(
            "doc-1",
            &[
                PageIndexEntry::text(1, "背景"),
                PageIndexEntry::text(2, "目标关键词"),
                PageIndexEntry::text(3, "结论"),
                PageIndexEntry::text(4, "附录"),
            ],
        )
        .expect("pages index");

    let pages = index.retrieve_context("doc-1", "目标关键词", 3).expect("context retrieves");
    let page_numbers =
        pages.iter().map(|page| page.page_number).collect::<std::collections::HashSet<_>>();
    assert_eq!(page_numbers, std::collections::HashSet::from([1, 2, 3]));
}
