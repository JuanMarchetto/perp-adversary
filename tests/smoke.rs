use percolator::v16::{
    v16_domain_count_for_market_slots, EngineAssetSlotV16Account, Market,
    MarketGroupV16HeaderAccount, MarketGroupV16ViewMut, PortfolioAccountV16Account,
    PortfolioSourceDomainV16Account, PortfolioV16ViewMut, ProvenanceHeaderV16,
    ProvenanceHeaderV16Account, V16Config,
};

fn market(slots: u32, price: u64) -> (MarketGroupV16HeaderAccount, Vec<Market<u64>>) {
    let cfg = V16Config::public_user_fund_with_market_slots(slots as u16, slots, 0, 10);
    let mut header = MarketGroupV16HeaderAccount::new_dynamic([1u8; 32], cfg, slots, 0).unwrap();
    let mut markets = (0..slots)
        .map(|i| Market::new(i as u64, EngineAssetSlotV16Account::default()))
        .collect::<Vec<_>>();
    {
        let mut v = MarketGroupV16ViewMut::new(&mut header, &mut markets);
        for i in 0..slots as usize {
            v.activate_empty_market_not_atomic(i as u32, price, (i + 1) as u64)
                .unwrap();
        }
        v.validate_shape().unwrap();
    }
    (header, markets)
}

fn account(
    slots: u32,
    seed: u8,
) -> (
    PortfolioAccountV16Account,
    Vec<PortfolioSourceDomainV16Account>,
) {
    let h = ProvenanceHeaderV16Account::from_runtime(&ProvenanceHeaderV16::new(
        [1u8; 32], [seed; 32], [3u8; 32],
    ));
    let acct = PortfolioAccountV16Account::try_empty(h).unwrap();
    let domains = vec![
        PortfolioSourceDomainV16Account::default();
        v16_domain_count_for_market_slots(slots).unwrap()
    ];
    (acct, domains)
}

#[test]
fn deposit_then_withdraw_roundtrip_moves_capital() {
    let (mut mh, mut mk) = market(1, 100);
    let (mut ah, mut sd) = account(1, 2);

    let mut mv = MarketGroupV16ViewMut::new(&mut mh, &mut mk);
    let mut av = PortfolioV16ViewMut::new(&mut ah, &mut sd);

    mv.deposit_not_atomic(&mut av, 100).unwrap();
    assert_eq!(
        av.header.capital.get(),
        100,
        "deposit should credit account capital"
    );

    mv.withdraw_not_atomic(&mut av, 40).unwrap();
    assert_eq!(
        av.header.capital.get(),
        60,
        "withdraw should debit account capital"
    );
}
